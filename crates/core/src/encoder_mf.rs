//! H.264 on the graphics card through a Media Foundation hardware encoder
//! (NVENC, AMD AMF, Intel Quick Sync all register one).
//!
//! Hardware encoders are asynchronous MFTs: they announce through events when
//! they take input and when output is ready. The capture loop is synchronous,
//! so `encode` feeds one frame and waits for its output. Anything unexpected
//! is an error, and the caller falls back to OpenH264.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use openh264::formats::YUVBuffer;
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;

/// Set once a hardware encoder failed; later sessions of this process use
/// OpenH264 straight away instead of failing again.
static GAVE_UP: AtomicBool = AtomicBool::new(false);

pub(crate) fn give_up() {
    GAVE_UP.store(true, Ordering::Relaxed);
}

/// The first frame may take a while (driver start); later ones come in a few ms.
const FIRST_OUTPUT: Duration = Duration::from_secs(2);
const OUTPUT: Duration = Duration::from_millis(250);

pub(crate) struct Hardware {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    codec: ICodecAPI,
    activate: IMFActivate,
    width: u32,
    height: u32,
    frame_duration: i64,
    time: i64,
    /// `METransformNeedInput` events not yet answered with a frame.
    wanted: u32,
    /// Output buffer size, if the encoder wants us to allocate samples.
    output_size: Option<u32>,
    frames: u64,
    scratch: Option<YUVBuffer>,
}

impl Hardware {
    /// `None` if there is no usable hardware encoder; the reason goes to the log.
    pub(crate) fn new(width: u32, height: u32, fps: f32, bitrate: u32) -> Option<Self> {
        if GAVE_UP.load(Ordering::Relaxed) || std::env::var_os("CTXREMOTE_SOFTWARE_ENCODER").is_some() {
            return None;
        }
        match unsafe { Self::open(width, height, fps, bitrate) } {
            Ok(Some(hardware)) => Some(hardware),
            Ok(None) => {
                tracing::debug!("Kein Grafikkarten-Encoder gefunden, kodiere mit dem Prozessor");
                give_up();
                None
            }
            Err(e) => {
                tracing::info!("Grafikkarten-Encoder nicht nutzbar, kodiere mit dem Prozessor: {e:#}");
                give_up();
                None
            }
        }
    }

    unsafe fn open(width: u32, height: u32, fps: f32, bitrate: u32) -> Result<Option<Self>> {
        unsafe {
            // Already initialised threads report S_FALSE or RPC_E_CHANGED_MODE; both are fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            static STARTUP: Once = Once::new();
            let mut started = Ok(());
            STARTUP.call_once(|| started = MFStartup(MF_VERSION, MFSTARTUP_LITE));
            started.context("Media Foundation startet nicht")?;

            for activate in hardware_encoders()? {
                let name = friendly_name(&activate);
                match Self::start(&activate, width, height, fps, bitrate) {
                    Ok(hardware) => {
                        tracing::info!(encoder = %name, width, height, "Kodiere mit der Grafikkarte");
                        return Ok(Some(hardware));
                    }
                    Err(e) => {
                        tracing::info!(encoder = %name, "Grafikkarten-Encoder abgelehnt: {e:#}");
                        let _ = activate.ShutdownObject();
                    }
                }
            }
            Ok(None)
        }
    }

    unsafe fn start(activate: &IMFActivate, width: u32, height: u32, fps: f32, bitrate: u32) -> Result<Self> {
        unsafe {
            let transform: IMFTransform = activate.ActivateObject().context("ActivateObject")?;
            let attributes = transform.GetAttributes().context("GetAttributes")?;
            if attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 1 {
                bail!("kein asynchroner Encoder");
            }
            attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).context("ASYNC_UNLOCK")?;
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            let events: IMFMediaEventGenerator = transform.cast().context("IMFMediaEventGenerator")?;
            let codec: ICodecAPI = transform.cast().context("ICodecAPI")?;

            let fps_num = fps.round().max(1.0) as u32;
            // Encoders want the output type before the input type.
            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, pack(fps_num, 1))?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            // Baseline only: the viewers are configured for it, and phones decode it everywhere.
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            transform.SetOutputType(0, &output, 0).context("Ausgabeformat (H.264 Baseline)")?;

            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps_num, 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &input, 0).context("Eingabeformat (NV12)")?;

            // Best effort: not every encoder knows every setting.
            let set = |api: GUID, value: VARIANT| {
                if let Err(e) = codec.SetValue(&api, &value) {
                    tracing::debug!(?api, "Encoder-Einstellung nicht übernommen: {e}");
                }
            };
            set(CODECAPI_AVLowLatencyMode, VARIANT::from(true));
            set(CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32));
            set(CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(bitrate));
            set(CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
            // Keyframes come on request; a long GOP keeps a still screen cheap.
            set(CODECAPI_AVEncMPVGOPSize, VARIANT::from(fps_num * 60));

            let info = transform.GetOutputStreamInfo(0).context("GetOutputStreamInfo")?;
            let provides = MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32;
            let output_size = (info.dwFlags & provides == 0).then(|| info.cbSize.max(width * height * 3 / 2));

            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).context("BEGIN_STREAMING")?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).context("START_OF_STREAM")?;

            Ok(Self {
                transform,
                events,
                codec,
                activate: activate.clone(),
                width,
                height,
                frame_duration: (10_000_000 / fps_num) as i64,
                time: 0,
                wanted: 0,
                output_size,
                frames: 0,
                scratch: None,
            })
        }
    }

    pub(crate) fn set_bitrate(&mut self, bitrate: u32) -> bool {
        let value = VARIANT::from(bitrate);
        match unsafe { self.codec.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &value) } {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!("Bitrate am Grafikkarten-Encoder nicht geändert: {e}");
                false
            }
        }
    }

    /// Encodes one packed BGRA frame and appends its Annex B output to `out`.
    pub(crate) fn encode(&mut self, bgra: &[u8], keyframe: bool, out: &mut Vec<u8>) -> Result<()> {
        unsafe {
            let timeout = if self.frames == 0 { FIRST_OUTPUT } else { OUTPUT };
            let deadline = Instant::now() + timeout;
            // Output still pending from an earlier frame goes out first.
            self.drain(out)?;
            while self.wanted == 0 {
                self.wait_event(deadline, out)?;
            }
            if keyframe || self.frames == 0 {
                let _ = self.codec.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
            }
            let sample = self.input_sample(bgra)?;
            self.transform.ProcessInput(0, &sample, 0).context("ProcessInput")?;
            self.wanted -= 1;
            self.frames += 1;
            let before = out.len();
            while out.len() == before {
                self.wait_event(deadline, out)?;
            }
            Ok(())
        }
    }

    unsafe fn input_sample(&mut self, bgra: &[u8]) -> Result<IMFSample> {
        unsafe {
            let size = (self.width * self.height * 3 / 2) as usize;
            let buffer = MFCreateMemoryBuffer(size as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            let nv12 = std::slice::from_raw_parts_mut(data, size);
            crate::encoder::bgra_to_nv12(bgra, self.width, self.height, &mut self.scratch, nv12);
            buffer.Unlock()?;
            buffer.SetCurrentLength(size as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(self.time)?;
            sample.SetSampleDuration(self.frame_duration)?;
            self.time += self.frame_duration;
            Ok(sample)
        }
    }

    /// Handles queued events without waiting.
    unsafe fn drain(&mut self, out: &mut Vec<u8>) -> Result<()> {
        unsafe { while self.next_event(out)? {} }
        Ok(())
    }

    /// Handles at least one event, polling until `deadline`.
    unsafe fn wait_event(&mut self, deadline: Instant, out: &mut Vec<u8>) -> Result<()> {
        unsafe {
            loop {
                if self.next_event(out)? {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    bail!("Grafikkarten-Encoder antwortet nicht");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Handles one event; `false` if none was queued.
    unsafe fn next_event(&mut self, out: &mut Vec<u8>) -> Result<bool> {
        unsafe {
            let event = match self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(event) => event,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(false),
                Err(e) => return Err(e).context("GetEvent"),
            };
            let status = event.GetStatus()?;
            if status.is_err() {
                bail!("Encoder meldet Fehler {status:?}");
            }
            let kind = MF_EVENT_TYPE(event.GetType()? as i32);
            if kind == METransformNeedInput {
                self.wanted += 1;
            } else if kind == METransformHaveOutput {
                self.take_output(out)?;
            }
            Ok(true)
        }
    }

    unsafe fn take_output(&mut self, out: &mut Vec<u8>) -> Result<()> {
        unsafe {
            let sample = match self.output_size {
                Some(size) => {
                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&MFCreateMemoryBuffer(size)?)?;
                    Some(sample)
                }
                None => None,
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0;
            let result = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let sample = std::mem::ManuallyDrop::take(&mut buffers[0].pSample);
            drop(std::mem::ManuallyDrop::take(&mut buffers[0].pEvents));
            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // The encoder settled on its final output format; take it and go on.
                    let format = self.transform.GetOutputAvailableType(0, 0)?;
                    self.transform.SetOutputType(0, &format, 0)?;
                    return Ok(());
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) => return Err(e).context("ProcessOutput"),
            }
            let Some(sample) = sample else { bail!("Encoder lieferte kein Bild") };
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut data = std::ptr::null_mut();
            let mut length = 0u32;
            buffer.Lock(&mut data, None, Some(&mut length))?;
            let bytes = std::slice::from_raw_parts(data, length as usize);
            // Viewers need SPS and PPS before every keyframe; some encoders keep them out of band.
            if crate::encoder::contains_idr(bytes) && !has_sps(bytes) {
                if let Ok(header) = self.sequence_header() {
                    out.extend_from_slice(&header);
                }
            }
            out.extend_from_slice(bytes);
            buffer.Unlock()?;
            Ok(())
        }
    }

    unsafe fn sequence_header(&self) -> Result<Vec<u8>> {
        unsafe {
            let format = self.transform.GetOutputCurrentType(0)?;
            let size = format.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)?;
            let mut header = vec![0u8; size as usize];
            format.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)?;
            Ok(header)
        }
    }
}

impl Drop for Hardware {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activate.ShutdownObject();
        }
    }
}

fn pack(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn has_sps(annex_b: &[u8]) -> bool {
    annex_b.windows(4).any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] & 0x1f == 7)
}

/// Hardware H.264 encoders taking NV12, best first.
unsafe fn hardware_encoders() -> Result<Vec<IMFActivate>> {
    unsafe {
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut list,
            &mut count,
        )
        .context("MFTEnumEx")?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        // Take ownership of every entry, then free the array itself.
        let found = (0..count as usize).filter_map(|i| std::ptr::read(list.add(i))).collect();
        CoTaskMemFree(Some(list as *const _));
        Ok(found)
    }
}

unsafe fn friendly_name(activate: &IMFActivate) -> String {
    unsafe {
        let mut name = PWSTR::null();
        let mut length = 0;
        if activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut length).is_err() {
            return "unbekannt".into();
        }
        let text = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.0 as *const _));
        text
    }
}
