//! The viewer's microphone at the host: Opus packets (20 ms, 48 kHz mono)
//! decoded and played on the default speaker, so the people on both ends can
//! talk during a session (`ViewerMsg::Mic`).
//!
//! A short buffer absorbs jitter; when packets stop, playback ends after a
//! few seconds and the speaker is released.

use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use opus_rs::OpusDecoder;

use crate::audio::{FRAME, SAMPLE_RATE};

/// How much sound waits before playback starts (and after a gap).
pub const LEAD_MS: u32 = 60;
/// Without packets this long, playback ends.
pub const IDLE_MS: u64 = 4000;

/// Decodes the viewer's packets.
pub struct MicDecoder {
    decoder: OpusDecoder,
    pcm: Vec<f32>,
}

impl MicDecoder {
    pub fn new() -> Result<Self> {
        let decoder = OpusDecoder::new(SAMPLE_RATE as i32, 1).map_err(|e| anyhow!("Opus: {e}"))?;
        Ok(Self { decoder, pcm: vec![0.0; FRAME * 6] })
    }

    /// Mono samples at 48 kHz; empty for a packet that does not decode.
    pub fn decode(&mut self, packet: &[u8]) -> Vec<f32> {
        match self.decoder.decode(packet, FRAME * 6, &mut self.pcm) {
            Ok(n) => self.pcm[..n].to_vec(),
            Err(e) => {
                tracing::debug!("Mikrofon-Paket verworfen: {e}");
                Vec::new()
            }
        }
    }
}

/// Plays decoded sound on its own thread; ends when idle or dropped.
pub struct MicPlayer {
    decoder: MicDecoder,
    samples: Option<mpsc::Sender<Vec<f32>>>,
    thread: Option<JoinHandle<()>>,
}

impl MicPlayer {
    pub fn new() -> Result<Self> {
        Ok(Self { decoder: MicDecoder::new()?, samples: None, thread: None })
    }

    pub fn push(&mut self, packet: &[u8]) {
        let samples = self.decoder.decode(packet);
        if samples.is_empty() {
            return;
        }
        // (Re)start the speaker after it went idle.
        if self.thread.as_ref().is_none_or(|t| t.is_finished()) {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new().name("ctxremote-speaker".into()).spawn(move || platform::play(rx)) {
                Ok(thread) => {
                    self.samples = Some(tx);
                    self.thread = Some(thread);
                }
                Err(e) => tracing::warn!("Wiedergabe nicht startbar: {e}"),
            }
        }
        if let Some(tx) = &self.samples {
            let _ = tx.send(samples);
        }
    }
}

impl Drop for MicPlayer {
    fn drop(&mut self) {
        // Closing the channel ends the thread.
        self.samples = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::VecDeque;
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use anyhow::Result;
    use windows::core::GUID;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};

    use super::{IDLE_MS, LEAD_MS};
    use crate::audio::{Resampler, SAMPLE_RATE};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    pub fn play(rx: Receiver<Vec<f32>>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        if let Err(e) = render(&rx) {
            tracing::debug!("Wiedergabe beendet: {e:#}");
        }
    }

    fn render(rx: &Receiver<Vec<f32>>) -> Result<()> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mix = client.GetMixFormat()?;
            let base: WAVEFORMATEX = std::ptr::read_unaligned(mix);
            let float = match base.wFormatTag {
                WAVE_FORMAT_IEEE_FLOAT => true,
                WAVE_FORMAT_EXTENSIBLE => {
                    let ext: WAVEFORMATEXTENSIBLE = std::ptr::read_unaligned(mix.cast());
                    let sub = ext.SubFormat;
                    sub == SUBTYPE_IEEE_FLOAT
                }
                _ => false,
            };
            let (rate, channels, bytes) =
                (base.nSamplesPerSec, base.nChannels.max(1) as usize, (base.wBitsPerSample / 8).max(1) as usize);
            // 100 ms of buffer, refilled every 10 ms.
            let initialized = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, mix, None);
            CoTaskMemFree(Some(mix as *const _));
            initialized?;
            let size = client.GetBufferSize()? as usize;
            let renderer: IAudioRenderClient = client.GetService()?;
            tracing::info!(rate, channels, "Mikrofon der Gegenseite wird abgespielt");

            let mut resampler = Resampler::between(SAMPLE_RATE, rate);
            let mut queue: VecDeque<f32> = VecDeque::new();
            let lead = (rate * LEAD_MS / 1000) as usize;
            let mut started = false;
            let mut last = Instant::now();
            let mut stereo = Vec::new();
            loop {
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(samples) => {
                        last = Instant::now();
                        let frames: Vec<[f32; 2]> = samples.iter().map(|&v| [v, v]).collect();
                        stereo.clear();
                        resampler.process(&frames, &mut stereo);
                        queue.extend(stereo.iter().step_by(2));
                        // Far behind (e.g. after a stall): drop the oldest, keep it live.
                        while queue.len() > lead * 5 {
                            queue.pop_front();
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if last.elapsed() > Duration::from_millis(IDLE_MS) {
                    break;
                }
                if !started {
                    if queue.len() < lead {
                        continue;
                    }
                    client.Start()?;
                    started = true;
                }
                let free = size - client.GetCurrentPadding()? as usize;
                if free == 0 {
                    continue;
                }
                let data = renderer.GetBuffer(free as u32)?;
                let out = std::slice::from_raw_parts_mut(data, free * channels * bytes);
                for frame in out.chunks_exact_mut(channels * bytes) {
                    // Silence while waiting for the next packets.
                    let v = queue.pop_front().unwrap_or(0.0);
                    for slot in frame.chunks_exact_mut(bytes) {
                        write_sample(slot, v, float);
                    }
                }
                renderer.ReleaseBuffer(free as u32, 0)?;
                if queue.is_empty() {
                    // Start again with a lead once sound returns.
                    let _ = client.Stop();
                    let _ = client.Reset();
                    started = false;
                }
            }
            let _ = client.Stop();
            Ok(())
        }
    }

    fn write_sample(slot: &mut [u8], v: f32, float: bool) {
        let v = v.clamp(-1.0, 1.0);
        match (slot.len(), float) {
            (4, true) => slot.copy_from_slice(&v.to_le_bytes()),
            (2, _) => slot.copy_from_slice(&((v * 32767.0) as i16).to_le_bytes()),
            (3, _) => slot.copy_from_slice(&(((v * 8_388_607.0) as i32).to_le_bytes())[..3]),
            (4, false) => slot.copy_from_slice(&((v * 2_147_483_647.0) as i32).to_le_bytes()),
            _ => slot.fill(0),
        }
    }
}

#[cfg(not(windows))]
mod platform {
    /// No speaker output outside Windows; the samples are dropped.
    pub fn play(rx: std::sync::mpsc::Receiver<Vec<f32>>) {
        while rx.recv_timeout(std::time::Duration::from_millis(super::IDLE_MS)).is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opus_rs::{Application, OpusEncoder};

    /// A mono packet as the viewer's WebCodecs encoder makes it decodes to
    /// 20 ms of the same tone.
    #[test]
    fn decodes_mono_packets() {
        let mut encoder = OpusEncoder::new(48_000, 1, Application::Voip).unwrap();
        let mut decoder = MicDecoder::new().unwrap();
        let mut out = Vec::new();
        let mut packet = [0u8; 1500];
        for n in 0..25 {
            let frame: Vec<f32> = (0..FRAME)
                .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 440.0 * (n * FRAME + i) as f32 / 48_000.0).sin())
                .collect();
            let len = encoder.encode(&frame, FRAME, &mut packet).unwrap();
            let pcm = decoder.decode(&packet[..len]);
            assert_eq!(pcm.len(), FRAME);
            out.extend(pcm);
        }
        // Past the codec's start-up, the pitch is right.
        let tail = &out[FRAME * 5..];
        let crossings = tail.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        let expected = 2.0 * 440.0 * tail.len() as f64 / 48_000.0;
        assert!((crossings as f64 - expected).abs() < 6.0, "{crossings} statt {expected}");
    }
}
