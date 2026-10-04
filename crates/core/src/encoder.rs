//! H.264 encoding of captured BGRA frames.

use anyhow::{Context, Result};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile,
    RateControlMode, SpsPpsStrategy, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use openh264::OpenH264API;
use ctxremote_proto::session::Quality;

pub struct VideoEncoder {
    encoder: Encoder,
    /// The quality preset's bitrate; congestion control scales it down.
    base_bitrate: u32,
    bitrate: u32,
    yuv: YUVBuffer,
    width: u32,
    height: u32,
    out: Vec<u8>,
}

pub struct Encoded<'a> {
    pub data: &'a [u8],
    pub keyframe: bool,
}

impl VideoEncoder {
    /// H.264 needs even dimensions; callers crop the odd edge pixel away.
    pub fn new(width: u32, height: u32, fps: f32, quality: Quality) -> Result<Self> {
        let (width, height) = (width & !1, height & !1);
        let pixels = width as f32 * height as f32;
        // Roughly 0.1 bit per pixel at 30 fps keeps text crisp without flooding slow links.
        let (bits_per_pixel, min, max) = match quality {
            Quality::Speed => (0.04, 600_000.0, 6_000_000.0),
            Quality::Balanced => (0.1, 1_500_000.0, 20_000_000.0),
            Quality::Sharp => (0.2, 3_000_000.0, 40_000_000.0),
        };
        let bitrate = (pixels * fps * bits_per_pixel).clamp(min, max) as u32;
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
        Ok(Self {
            encoder,
            base_bitrate: bitrate,
            bitrate,
            yuv: YUVBuffer::new(width as usize, height as usize),
            width,
            height,
            out: Vec::with_capacity(512 * 1024),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Sets the bitrate to `factor` (0..=1) of the preset's. Takes effect with
    /// the next frame, without a keyframe.
    pub fn set_bitrate_factor(&mut self, factor: f32) {
        let bitrate = (self.base_bitrate as f32 * factor.clamp(0.0, 1.0)).max(250_000.0) as u32;
        if bitrate == self.bitrate {
            return;
        }
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
        if result == 0 {
            self.bitrate = bitrate;
        } else {
            tracing::debug!(result, "Bitrate konnte nicht geändert werden");
        }
    }

    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    pub fn force_keyframe(&mut self) {
        self.encoder.force_intra_frame();
    }

    /// Encodes a tightly packed BGRA frame of exactly the encoder's size.
    pub fn encode(&mut self, bgra: &[u8]) -> Result<Encoded<'_>> {
        let dims = (self.width as usize, self.height as usize);
        self.yuv.read_bgra8(BgraSliceU8::new(bgra, dims));
        let bitstream = self.encoder.encode(&self.yuv)?;
        self.out.clear();
        bitstream.write_vec(&mut self.out);
        let keyframe = contains_idr(&self.out);
        Ok(Encoded { data: &self.out, keyframe })
    }
}

/// Scans Annex B start codes for an IDR slice (NAL type 5).
fn contains_idr(annex_b: &[u8]) -> bool {
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
    fn first_frame_is_a_keyframe() {
        let mut enc = VideoEncoder::new(320, 240, 30.0, Quality::Balanced).unwrap();
        let frame = vec![128u8; 320 * 240 * 4];
        let first = enc.encode(&frame).unwrap();
        assert!(first.keyframe);
        assert!(!first.data.is_empty());
    }
}
