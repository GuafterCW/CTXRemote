//! H.264 encoding of captured BGRA frames.

use anyhow::{Context, Result};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile,
    RateControlMode, SpsPpsStrategy, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use openh264::OpenH264API;
use ctxremote_proto::session::Quality;

/// Encodes with the graphics card where one is available (Windows, Media
/// Foundation) and with OpenH264 otherwise. A graphics encoder that fails
/// mid-session hands over to OpenH264 with a keyframe; the stream stays
/// constrained baseline either way, so every viewer can decode it.
pub struct VideoEncoder {
    backend: Backend,
    /// Fallback to OpenH264 needs these; only Windows has a hardware path.
    #[cfg_attr(not(windows), allow(dead_code))]
    fps: f32,
    /// The quality preset's bitrate; congestion control scales it down.
    base_bitrate: u32,
    bitrate: u32,
    width: u32,
    height: u32,
    #[cfg_attr(not(windows), allow(dead_code))]
    force_key: bool,
    out: Vec<u8>,
}

enum Backend {
    Software(Software),
    #[cfg(windows)]
    Hardware(crate::encoder_mf::Hardware),
}

pub struct Encoded<'a> {
    pub data: &'a [u8],
    pub keyframe: bool,
}

impl VideoEncoder {
    /// H.264 needs even dimensions; callers crop the odd edge pixel away.
    pub fn new(width: u32, height: u32, fps: f32, quality: Quality) -> Result<Self> {
        let (width, height) = (width & !1, height & !1);
        let bitrate = preset_bitrate(width, height, fps, quality);
        #[cfg(windows)]
        let backend = match crate::encoder_mf::Hardware::new(width, height, fps, bitrate) {
            Some(hardware) => Backend::Hardware(hardware),
            None => Backend::Software(Software::new(width, height, fps, bitrate)?),
        };
        #[cfg(not(windows))]
        let backend = Backend::Software(Software::new(width, height, fps, bitrate)?);
        Ok(Self {
            backend,
            fps,
            base_bitrate: bitrate,
            bitrate,
            width,
            height,
            force_key: false,
            out: Vec::with_capacity(512 * 1024),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// "Grafikkarte" or "Prozessor", for the log.
    pub fn kind(&self) -> &'static str {
        match self.backend {
            Backend::Software(_) => "Prozessor",
            #[cfg(windows)]
            Backend::Hardware(_) => "Grafikkarte",
        }
    }

    /// Sets the bitrate to `factor` (0..=1) of the preset's. Takes effect with
    /// the next frame, without a keyframe.
    pub fn set_bitrate_factor(&mut self, factor: f32) {
        let bitrate = (self.base_bitrate as f32 * factor.clamp(0.0, 1.0)).max(250_000.0) as u32;
        if bitrate == self.bitrate {
            return;
        }
        let applied = match &mut self.backend {
            Backend::Software(software) => software.set_bitrate(bitrate),
            #[cfg(windows)]
            Backend::Hardware(hardware) => hardware.set_bitrate(bitrate),
        };
        if applied {
            self.bitrate = bitrate;
        }
    }

    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    pub fn force_keyframe(&mut self) {
        match &mut self.backend {
            Backend::Software(software) => software.encoder.force_intra_frame(),
            #[cfg(windows)]
            Backend::Hardware(_) => self.force_key = true,
        }
    }

    /// Encodes a tightly packed BGRA frame of exactly the encoder's size.
    pub fn encode(&mut self, bgra: &[u8]) -> Result<Encoded<'_>> {
        self.out.clear();
        #[cfg(windows)]
        if let Backend::Hardware(hardware) = &mut self.backend {
            match hardware.encode(bgra, std::mem::take(&mut self.force_key), &mut self.out) {
                Ok(()) => {
                    let keyframe = contains_idr(&self.out);
                    return Ok(Encoded { data: &self.out, keyframe });
                }
                Err(e) => {
                    tracing::warn!("Grafikkarten-Encoder ausgefallen, weiter mit dem Prozessor: {e:#}");
                    crate::encoder_mf::give_up();
                    // A fresh OpenH264 encoder starts with a keyframe.
                    self.backend = Backend::Software(Software::new(self.width, self.height, self.fps, self.bitrate)?);
                    self.out.clear();
                }
            }
        }
        match &mut self.backend {
            Backend::Software(software) => software.encode(bgra, &mut self.out)?,
            #[cfg(windows)]
            Backend::Hardware(_) => unreachable!("handled above"),
        }
        let keyframe = contains_idr(&self.out);
        Ok(Encoded { data: &self.out, keyframe })
    }
}

/// Roughly 0.1 bit per pixel at 30 fps keeps text crisp without flooding slow links.
fn preset_bitrate(width: u32, height: u32, fps: f32, quality: Quality) -> u32 {
    let pixels = width as f32 * height as f32;
    let (bits_per_pixel, min, max) = match quality {
        Quality::Speed => (0.04, 600_000.0, 6_000_000.0),
        Quality::Balanced => (0.1, 1_500_000.0, 20_000_000.0),
        Quality::Sharp => (0.2, 3_000_000.0, 40_000_000.0),
    };
    (pixels * fps * bits_per_pixel).clamp(min, max) as u32
}

struct Software {
    encoder: Encoder,
    yuv: YUVBuffer,
    width: u32,
    height: u32,
}

impl Software {
    fn new(width: u32, height: u32, fps: f32, bitrate: u32) -> Result<Self> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .profile(Profile::Baseline)
            .complexity(Complexity::Low)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(fps))
            // Rate control only works when the encoder may drop frames.
            .skip_frames(true)
            // Neither is supported for screen content; set explicitly to keep the log quiet.
            .adaptive_quantization(false)
            .background_detection(false)
            .sps_pps_strategy(SpsPpsStrategy::ConstantId)
            .intra_frame_period(IntraFramePeriod::from_num_frames(0))
            .num_threads(0);
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .context("H.264-Encoder konnte nicht gestartet werden")?;
        Ok(Self { encoder, yuv: YUVBuffer::new(width as usize, height as usize), width, height })
    }

    fn set_bitrate(&mut self, bitrate: u32) -> bool {
        let mut info = openh264_sys2::SBitrateInfo {
            iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
            iBitrate: bitrate as i32,
        };
        // SAFETY: the encoder is initialised; SetOption copies the struct.
        let result = unsafe {
            self.encoder
                .raw_api()
                .set_option(openh264_sys2::ENCODER_OPTION_BITRATE, std::ptr::addr_of_mut!(info).cast())
        };
        if result != 0 {
            tracing::debug!(result, "Bitrate konnte nicht geändert werden");
        }
        result == 0
    }

    fn encode(&mut self, bgra: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let dims = (self.width as usize, self.height as usize);
        self.yuv.read_bgra8(BgraSliceU8::new(bgra, dims));
        let bitstream = self.encoder.encode(&self.yuv)?;
        bitstream.write_vec(out);
        Ok(())
    }
}

/// Converts packed BGRA to NV12 (BT.601, limited range), the same colours
/// OpenH264 produces. `scratch` keeps the planar intermediate between calls.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn bgra_to_nv12(bgra: &[u8], width: u32, height: u32, scratch: &mut Option<YUVBuffer>, nv12: &mut [u8]) {
    use openh264::formats::YUVSource;
    let (w, h) = (width as usize, height as usize);
    let yuv = scratch.get_or_insert_with(|| YUVBuffer::new(w, h));
    if yuv.dimensions() != (w, h) {
        *yuv = YUVBuffer::new(w, h);
    }
    yuv.read_bgra8(BgraSliceU8::new(bgra, (w, h)));
    let (luma, chroma) = nv12.split_at_mut(w * h);
    luma.copy_from_slice(&yuv.y()[..w * h]);
    for ((pair, u), v) in chroma.chunks_exact_mut(2).zip(yuv.u()).zip(yuv.v()) {
        pair[0] = *u;
        pair[1] = *v;
    }
}

/// Scans Annex B start codes for an IDR slice (NAL type 5).
pub(crate) fn contains_idr(annex_b: &[u8]) -> bool {
    annex_b
        .windows(4)
        .any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] & 0x1f == 5)
}

/// Copies a pitched BGRA surface into a packed buffer, dropping odd edge pixels.
pub fn pack_bgra(src: &[u8], pitch: usize, width: u32, height: u32, dst: &mut Vec<u8>) {
    let (w, h) = ((width & !1) as usize, (height & !1) as usize);
    dst.resize(w * h * 4, 0);
    for (row, out) in dst.chunks_exact_mut(w * 4).enumerate().take(h) {
        let start = row * pitch;
        out.copy_from_slice(&src[start..start + w * 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_changes_without_keyframe() {
        let mut enc = VideoEncoder::new(320, 240, 30.0, Quality::Sharp).unwrap();
        let mut frame = vec![128u8; 320 * 240 * 4];
        assert!(enc.encode(&frame).unwrap().keyframe);
        let base = enc.bitrate();
        enc.set_bitrate_factor(0.5);
        assert_eq!(enc.bitrate(), base / 2);
        frame[0] = 0;
        assert!(!enc.encode(&frame).unwrap().keyframe);
    }

    #[test]
    fn nv12_interleaves_the_chroma_planes() {
        // Pure blue: Y 41, U 240, V 110 in BT.601 limited range.
        let frame: Vec<u8> = [255u8, 0, 0, 255].repeat(4 * 2);
        let mut nv12 = vec![0u8; 4 * 2 * 3 / 2];
        bgra_to_nv12(&frame, 4, 2, &mut None, &mut nv12);
        let (luma, chroma) = nv12.split_at(8);
        assert!(luma.iter().all(|&y| (y as i32 - 41).abs() <= 2), "{luma:?}");
        assert_eq!(chroma.len(), 4);
        assert!((chroma[0] as i32 - 240).abs() <= 2 && (chroma[1] as i32 - 110).abs() <= 2, "{chroma:?}");
        assert_eq!(chroma[0..2], chroma[2..4]);
    }

    #[test]
    fn first_frame_is_a_keyframe() {
        let mut enc = VideoEncoder::new(320, 240, 30.0, Quality::Balanced).unwrap();
        let frame = vec![128u8; 320 * 240 * 4];
        let first = enc.encode(&frame).unwrap();
        assert!(first.keyframe);
        assert!(!first.data.is_empty());
    }
}
