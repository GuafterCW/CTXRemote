//! The host's sound for the viewer: what the computer plays (WASAPI
//! loopback of the default output), as Opus packets of 20 ms, 48 kHz stereo.
//! The viewer decodes them with WebCodecs in the session window.
//!
//! Only while a viewer asked for it (`ViewerMsg::SetAudio`). When nothing
//! plays, Windows delivers nothing and no packets go out.

use anyhow::{anyhow, Result};
use opus_rs::{Application, OpusEncoder};
use tokio::sync::mpsc;

pub const SAMPLE_RATE: u32 = 48_000;
/// 20 ms at 48 kHz.
pub const FRAME: usize = 960;
const BITRATE: i32 = 96_000;

/// Interleaved stereo at 48 kHz in, Opus packets out.
pub struct OpusFrames {
    encoder: OpusEncoder,
    pending: Vec<f32>,
}

impl OpusFrames {
    pub fn new() -> Result<Self> {
        // The low-delay mode skips the speech coder's lookahead; desktop sound
        // is mostly music and effects anyway.
        let mut encoder =
            OpusEncoder::new(SAMPLE_RATE as i32, 2, Application::RestrictedLowDelay).map_err(|e| anyhow!("Opus: {e}"))?;
        encoder.bitrate_bps = BITRATE;
        Ok(Self { encoder, pending: Vec::with_capacity(FRAME * 4) })
    }

    /// Adds samples; calls `out` for every complete 20 ms packet.
    pub fn push(&mut self, stereo: &[f32], mut out: impl FnMut(Vec<u8>)) {
        self.pending.extend_from_slice(stereo);
        let mut packet = [0u8; 1500];
        let mut start = 0;
        while self.pending.len() - start >= FRAME * 2 {
            let frame = &self.pending[start..start + FRAME * 2];
            match self.encoder.encode(frame, FRAME, &mut packet) {
                Ok(n) => out(packet[..n].to_vec()),
                Err(e) => tracing::debug!("Opus-Frame verworfen: {e}"),
            }
            start += FRAME * 2;
        }
        self.pending.drain(..start);
    }
}

/// Linear resampling of stereo frames to 48 kHz, carried across calls.
pub struct Resampler {
    step: f64,
    pos: f64,
    buf: Vec<[f32; 2]>,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Self {
        Self::between(input_rate, SAMPLE_RATE)
    }

    /// From `from` Hz to `to` Hz, e.g. 48 kHz sound for a 44.1 kHz speaker.
    pub fn between(from: u32, to: u32) -> Self {
        Self { step: from as f64 / to.max(1) as f64, pos: 0.0, buf: Vec::new() }
    }

    /// Appends the resampled `input` to `out` as interleaved stereo.
    pub fn process(&mut self, input: &[[f32; 2]], out: &mut Vec<f32>) {
        if self.step == 1.0 {
            out.extend(input.iter().flatten());
            return;
        }
        self.buf.extend_from_slice(input);
        while self.pos + 1.0 < self.buf.len() as f64 {
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            let (a, b) = (self.buf[i], self.buf[i + 1]);
            out.push(a[0] + (b[0] - a[0]) * f);
            out.push(a[1] + (b[1] - a[1]) * f);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.pos -= used as f64;
    }
}

/// Captures and encodes on its own thread until dropped.
pub struct AudioCapture {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioCapture {
    /// Starts sending packets to `out`; full channels drop packets rather than
    /// building up delay.
    pub fn start(out: mpsc::Sender<Vec<u8>>) -> Result<Self> {
        if !cfg!(windows) {
            anyhow::bail!("Ton gibt es nur unter Windows");
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = std::thread::Builder::new().name("ctxremote-audio".into()).spawn({
            let stop = stop.clone();
            move || platform::run(&stop, &out)
        })?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use anyhow::Result;
    use tokio::sync::mpsc;
    use windows::core::GUID;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
        WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};

    use super::{OpusFrames, Resampler};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    /// Captures until `stop`; starts over after errors, e.g. when the user
    /// switches the output device.
    pub fn run(stop: &AtomicBool, out: &mpsc::Sender<Vec<u8>>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while !stop.load(Ordering::Relaxed) {
            if let Err(e) = capture(stop, out) {
                tracing::debug!("Tonaufnahme unterbrochen: {e:#}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }

    struct Format {
        rate: u32,
        channels: usize,
        bytes: usize,
        float: bool,
    }

    fn capture(stop: &AtomicBool, out: &mpsc::Sender<Vec<u8>>) -> Result<()> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mix = client.GetMixFormat()?;
            let format = {
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
                Format {
                    rate: base.nSamplesPerSec,
                    channels: base.nChannels.max(1) as usize,
                    bytes: (base.wBitsPerSample / 8).max(1) as usize,
                    float,
                }
            };
            // 200 ms of buffer; we read every 10 ms.
            let initialized = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, mix, None);
            CoTaskMemFree(Some(mix as *const _));
            initialized?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            tracing::info!(rate = format.rate, channels = format.channels, "Tonaufnahme läuft");

            let mut resampler = Resampler::new(format.rate);
            let mut encoder = OpusFrames::new()?;
            let (mut frames, mut stereo) = (Vec::new(), Vec::new());
            let result = loop {
                if stop.load(Ordering::Relaxed) {
                    break Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
                let mut failed = None;
                loop {
                    let available = match capture.GetNextPacketSize() {
                        Ok(n) => n,
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    };
                    if available == 0 {
                        break;
                    }
                    let (mut data, mut count, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    if let Err(e) = capture.GetBuffer(&mut data, &mut count, &mut flags, None, None) {
                        failed = Some(e);
                        break;
                    }
                    frames.clear();
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let block = format.channels * format.bytes;
                    let bytes = std::slice::from_raw_parts(data, count as usize * block);
                    for frame in bytes.chunks_exact(block) {
                        if silent {
                            frames.push([0.0, 0.0]);
                            continue;
                        }
                        let left = sample(&frame[..format.bytes], format.float);
                        let right = if format.channels > 1 { sample(&frame[format.bytes..2 * format.bytes], format.float) } else { left };
                        frames.push([left, right]);
                    }
                    let _ = capture.ReleaseBuffer(count);
                    stereo.clear();
                    resampler.process(&frames, &mut stereo);
                    encoder.push(&stereo, |packet| {
                        let _ = out.try_send(packet);
                    });
                }
                if let Some(e) = failed {
                    break Err(e.into());
                }
            };
            let _ = client.Stop();
            result
        }
    }

    /// One sample as f32: 32-bit float, or 16/24/32-bit integer.
    fn sample(bytes: &[u8], float: bool) -> f32 {
        match (bytes.len(), float) {
            (4, true) => f32::from_le_bytes(bytes.try_into().unwrap()),
            (2, _) => i16::from_le_bytes(bytes.try_into().unwrap()) as f32 / 32768.0,
            (3, _) => (i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) as f32 / 8_388_608.0,
            (4, false) => i32::from_le_bytes(bytes.try_into().unwrap()) as f32 / 2_147_483_648.0,
            _ => 0.0,
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::sync::atomic::AtomicBool;

    pub fn run(_stop: &AtomicBool, _out: &tokio::sync::mpsc::Sender<Vec<u8>>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, frames: usize, rate: u32) -> Vec<[f32; 2]> {
        (0..frames)
            .map(|i| {
                let v = 0.4 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin();
                [v, -v]
            })
            .collect()
    }

    #[test]
    fn resampling_keeps_pitch_and_length() {
        let mut resampler = Resampler::new(44_100);
        let mut out = Vec::new();
        // In uneven chunks, as WASAPI delivers them.
        for chunk in tone(440.0, 44_100, 44_100).chunks(441 + 7) {
            resampler.process(chunk, &mut out);
        }
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!((left.len() as i64 - 48_000).abs() < 10, "{}", left.len());
        let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((crossings as i64 - 880).abs() <= 2, "{crossings}");
    }

    #[test]
    fn packets_of_twenty_milliseconds() {
        let mut frames = OpusFrames::new().unwrap();
        let mut packets = Vec::new();
        let stereo: Vec<f32> = tone(440.0, FRAME * 10 + 100, SAMPLE_RATE).into_iter().flatten().collect();
        for chunk in stereo.chunks(333 * 2) {
            frames.push(chunk, |p| packets.push(p));
        }
        assert_eq!(packets.len(), 10);
        // Opus TOC byte: CELT-only fullband, 20 ms frames (config 31).
        assert!(packets.iter().all(|p| p[0] >> 3 == 31), "{:x}", packets[0][0]);
        let average = packets.iter().map(Vec::len).sum::<usize>() / packets.len();
        assert!((150..400).contains(&average), "{average} Bytes");
    }
}
