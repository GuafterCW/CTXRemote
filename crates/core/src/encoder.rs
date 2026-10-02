//! H.264 encoding of captured BGRA frames.

use anyhow::{Context, Result};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile,
    RateControlMode, SpsPpsStrategy, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use openh264::OpenH264API;

pub struct VideoEncoder {
    encoder: Encoder,
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
    pub fn new(width: u32, height: u32, fps: f32) -> Result<Self> {
        let (width, height) = (width & !1, height & !1);
        let pixels = width as f32 * height as f32;
        // Roughly 0.1 bit per pixel at 30 fps keeps text crisp without flooding slow links.
        let bitrate = (pixels * fps * 0.1).clamp(1_500_000.0, 20_000_000.0) as u32;
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
            yuv: YUVBuffer::new(width as usize, height as usize),
            width,
            height,
            out: Vec::with_capacity(512 * 1024),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
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
    fn first_frame_is_a_keyframe() {
        let mut enc = VideoEncoder::new(320, 240, 30.0).unwrap();
        let frame = vec![128u8; 320 * 240 * 4];
        let first = enc.encode(&frame).unwrap();
        assert!(first.keyframe);
        assert!(!first.data.is_empty());
    }
}
