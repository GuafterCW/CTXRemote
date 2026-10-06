//! Records a session's picture as an MP4 file, as it arrives (H.264, no
//! re-encoding), and its sound (Opus) if it was on when the file began.
//!
//! The file is fragmented MP4: a header up front, then one fragment per
//! frame. It plays up to the last frame written, even if the app ends
//! without closing it. A new screen size needs a new file, so the caller
//! starts another one then.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::VideoFrame;

/// Media time units per second.
const TIMESCALE: u32 = 90_000;
/// Duration of the last frame, which has no successor to measure against.
const LAST_FRAME: u32 = TIMESCALE / 30;
/// Sound: 48 kHz, one Opus packet per 20 ms.
const AUDIO_RATE: u32 = 48_000;
const AUDIO_PACKET: u32 = AUDIO_RATE / 50;
const VIDEO_TRACK: u32 = 1;
const AUDIO_TRACK: u32 = 2;

pub struct Recorder {
    path: PathBuf,
    out: BufWriter<File>,
    width: u32,
    height: u32,
    /// Waits for its duration until the next frame comes (with its timestamp).
    pending: Option<(Vec<u8>, bool, u64)>,
    /// Media time of the next sample.
    time: u64,
    sequence: u32,
    /// The sound track exists (decided when the file began).
    audio: bool,
    /// Media time of the next sound packet, in samples.
    audio_time: u64,
    /// When the file began, to place sound after a silence.
    started: Instant,
}

impl Recorder {
    /// Starts a file with the stream's settings from `keyframe`, which also
    /// becomes its first frame.
    pub fn start(path: &Path, keyframe: &VideoFrame) -> Result<Self> {
        Self::start_with(path, keyframe, false)
    }

    /// Like [`Recorder::start`]; with `audio`, the file gets a sound track
    /// for [`Recorder::push_audio`].
    pub fn start_with(path: &Path, keyframe: &VideoFrame, audio: bool) -> Result<Self> {
        if !keyframe.keyframe {
            bail!("Eine Aufnahme beginnt mit einem Schlüsselbild");
        }
        let nals = split_annex_b(&keyframe.data);
        let sps = nals.iter().find(|n| nal_type(n) == 7).context("Schlüsselbild ohne SPS")?;
        let pps = nals.iter().find(|n| nal_type(n) == 8).context("Schlüsselbild ohne PPS")?;
        if sps.len() < 4 {
            bail!("SPS zu kurz");
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File::create(path).with_context(|| format!("{} nicht anlegbar", path.display()))?;
        let mut out = BufWriter::new(file);
        out.write_all(&ftyp())?;
        out.write_all(&moov(keyframe.width, keyframe.height, sps, pps, audio))?;
        let mut recorder = Self {
            path: path.to_path_buf(),
            out,
            width: keyframe.width,
            height: keyframe.height,
            pending: None,
            time: 0,
            sequence: 0,
            audio,
            audio_time: 0,
            started: Instant::now(),
        };
        recorder.push(keyframe)?;
        Ok(recorder)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether `frame` fits this file; a new size needs a new one.
    pub fn fits(&self, frame: &VideoFrame) -> bool {
        frame.width == self.width && frame.height == self.height
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<()> {
        let sample = to_avcc(&frame.data);
        if sample.is_empty() {
            return Ok(());
        }
        if let Some((data, sync, at)) = self.pending.take() {
            // The host's capture times, so the video runs at the real pace.
            let micros = frame.timestamp_us.saturating_sub(at);
            let ticks = micros.saturating_mul(TIMESCALE as u64) / 1_000_000;
            let ticks = if ticks == 0 || ticks > TIMESCALE as u64 * 60 { LAST_FRAME as u64 } else { ticks };
            self.write_sample(&data, sync, ticks as u32)?;
        }
        self.pending = Some((sample, frame.keyframe, frame.timestamp_us));
        Ok(())
    }

    /// One Opus packet of sound (20 ms, 48 kHz stereo). Packets stop during
    /// silence, so time that passed without them is skipped over.
    pub fn push_audio(&mut self, packet: &[u8]) -> Result<()> {
        if !self.audio || packet.is_empty() {
            return Ok(());
        }
        let now = (self.started.elapsed().as_secs_f64() * AUDIO_RATE as f64) as u64;
        if now > self.audio_time + 5 * AUDIO_PACKET as u64 {
            self.audio_time = now;
        }
        self.sequence += 1;
        let moof = moof(AUDIO_TRACK, self.sequence, self.audio_time, AUDIO_PACKET, packet.len() as u32, true);
        self.write_fragment(&moof, packet)?;
        self.audio_time += AUDIO_PACKET as u64;
        Ok(())
    }

    /// Writes the last frame and closes the file; returns its path.
    pub fn finish(mut self) -> Result<PathBuf> {
        if let Some((data, sync, _)) = self.pending.take() {
            self.write_sample(&data, sync, LAST_FRAME)?;
        }
        self.out.flush()?;
        Ok(self.path)
    }

    fn write_sample(&mut self, data: &[u8], sync: bool, duration: u32) -> Result<()> {
        self.sequence += 1;
        let moof = moof(VIDEO_TRACK, self.sequence, self.time, duration, data.len() as u32, sync);
        self.write_fragment(&moof, data)?;
        self.time += duration as u64;
        Ok(())
    }

    fn write_fragment(&mut self, moof: &[u8], data: &[u8]) -> Result<()> {
        self.out.write_all(moof)?;
        self.out.write_all(&(data.len() as u32 + 8).to_be_bytes())?;
        self.out.write_all(b"mdat")?;
        self.out.write_all(data)?;
        // Each sample reaches the disk, so a crash loses at most the newest.
        self.out.flush()?;
        Ok(())
    }
}

/// Android's shared storage, where apps may create files in the standard
/// media folders without a permission (Android 11 and later); the gallery
/// lists them.
#[cfg(target_os = "android")]
pub(crate) const ANDROID_STORAGE: &str = "/storage/emulated/0";

/// Where recordings go by default: the user's video folder.
#[cfg(target_os = "android")]
pub fn default_dir() -> PathBuf {
    Path::new(ANDROID_STORAGE).join("Movies").join("CTXRemote")
}

/// Where screenshots of sessions go.
#[cfg(target_os = "android")]
pub fn pictures_dir() -> PathBuf {
    Path::new(ANDROID_STORAGE).join("Pictures").join("CTXRemote")
}

/// Where recordings go by default: the user's video folder.
#[cfg(not(target_os = "android"))]
pub fn default_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.video_dir().map(Path::to_path_buf).or_else(|| Some(d.home_dir().join("Videos"))))
        .unwrap_or_else(std::env::temp_dir)
        .join("CTXRemote")
}

/// Where screenshots of sessions go.
#[cfg(not(target_os = "android"))]
pub fn pictures_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.picture_dir().map(Path::to_path_buf).or_else(|| Some(d.home_dir().join("Pictures"))))
        .unwrap_or_else(std::env::temp_dir)
        .join("CTXRemote")
}

/// A file name part made from a device's name.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() { "Sitzung".into() } else { trimmed.chars().take(40).collect() }
}

// ---- H.264 elementary stream ----

/// NAL units of an Annex B stream (start codes `00 00 01` / `00 00 00 01`).
fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::new();
    for (k, &start) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&next| next - 3);
        // The zero of a four-byte start code belongs to no NAL.
        while end > start && data[end - 1] == 0 && starts.get(k + 1).is_some() {
            end -= 1;
        }
        if end > start {
            nals.push(&data[start..end]);
        }
    }
    nals
}

fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1f)
}

/// Length-prefixed NAL units as MP4 stores them; parameter sets and
/// delimiters stay out (they are in the header).
fn to_avcc(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for nal in split_annex_b(data) {
        if matches!(nal_type(nal), 7 | 8 | 9) {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

// ---- MP4 boxes ----

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A "full box": version and flags before the body.
fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut head = vec![version];
    head.extend_from_slice(&flags.to_be_bytes()[1..]);
    head.extend_from_slice(body);
    boxed(kind, &head)
}

fn concat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn matrix() -> Vec<u8> {
    MATRIX.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn ftyp() -> Vec<u8> {
    let mut body = b"isom".to_vec();
    body.extend_from_slice(&0x200u32.to_be_bytes());
    for brand in [b"isom", b"iso6", b"avc1", b"mp41"] {
        body.extend_from_slice(brand);
    }
    boxed(b"ftyp", &body)
}

fn moov(width: u32, height: u32, sps: &[u8], pps: &[u8], audio: bool) -> Vec<u8> {
    let mut mvhd = Vec::new();
    mvhd.extend_from_slice(&[0; 8]); // creation, modification
    mvhd.extend_from_slice(&1000u32.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration: in the fragments
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd.extend_from_slice(&[0; 10]);
    mvhd.extend_from_slice(&matrix());
    mvhd.extend_from_slice(&[0; 24]);
    mvhd.extend_from_slice(&3u32.to_be_bytes()); // next track ID

    let mut tkhd = Vec::new();
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&1u32.to_be_bytes()); // track ID
    tkhd.extend_from_slice(&[0; 4]);
    tkhd.extend_from_slice(&0u32.to_be_bytes()); // duration
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&[0; 4]); // layer, alternate group
    tkhd.extend_from_slice(&[0; 4]); // volume (video), reserved
    tkhd.extend_from_slice(&matrix());
    tkhd.extend_from_slice(&(width << 16).to_be_bytes());
    tkhd.extend_from_slice(&(height << 16).to_be_bytes());

    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&[0; 8]);
    mdhd.extend_from_slice(&TIMESCALE.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&0x55c4u16.to_be_bytes()); // "und"
    mdhd.extend_from_slice(&[0; 2]);

    let mut hdlr = vec![0; 4];
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0; 12]);
    hdlr.extend_from_slice(b"CTXRemote\0");

    let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(sps);
    avcc.push(1);
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(pps);

    let mut avc1 = vec![0; 6];
    avc1.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    avc1.extend_from_slice(&[0; 16]);
    avc1.extend_from_slice(&(width as u16).to_be_bytes());
    avc1.extend_from_slice(&(height as u16).to_be_bytes());
    avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
    avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1.extend_from_slice(&[0; 4]);
    avc1.extend_from_slice(&1u16.to_be_bytes()); // frames per sample
    avc1.extend_from_slice(&[0; 32]); // compressor name
    avc1.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
    avc1.extend_from_slice(&0xffffu16.to_be_bytes());
    avc1.extend_from_slice(&boxed(b"avcC", &avcc));

    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&boxed(b"avc1", &avc1));
    let empty = 0u32.to_be_bytes();
    let stbl = boxed(
        b"stbl",
        &concat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &empty),
            full(b"stsc", 0, 0, &empty),
            full(b"stsz", 0, 0, &[0; 8]),
            full(b"stco", 0, 0, &empty),
        ]),
    );
    let mut dref = 1u32.to_be_bytes().to_vec();
    dref.extend_from_slice(&full(b"url ", 0, 1, &[]));
    let minf = boxed(
        b"minf",
        &concat(&[full(b"vmhd", 0, 1, &[0; 8]), boxed(b"dinf", &full(b"dref", 0, 0, &dref)), stbl]),
    );
    let mdia = boxed(b"mdia", &concat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]));
    let trak = boxed(b"trak", &concat(&[full(b"tkhd", 0, 3, &tkhd), mdia]));

    let trex = |track: u32| {
        let mut body = track.to_be_bytes().to_vec();
        body.extend_from_slice(&1u32.to_be_bytes()); // sample description
        body.extend_from_slice(&[0; 12]);
        full(b"trex", 0, 0, &body)
    };
    let mut parts = vec![full(b"mvhd", 0, 0, &mvhd), trak];
    let mut extends = vec![trex(VIDEO_TRACK)];
    if audio {
        parts.push(audio_trak());
        extends.push(trex(AUDIO_TRACK));
    }
    parts.push(boxed(b"mvex", &concat(&extends)));
    boxed(b"moov", &concat(&parts))
}

/// The sound track: Opus, 48 kHz stereo.
fn audio_trak() -> Vec<u8> {
    let mut tkhd = Vec::new();
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&AUDIO_TRACK.to_be_bytes());
    tkhd.extend_from_slice(&[0; 4]);
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&[0, 0, 0, 1]); // layer 0, alternate group 1
    tkhd.extend_from_slice(&[0x01, 0x00, 0, 0]); // volume 1.0
    tkhd.extend_from_slice(&matrix());
    tkhd.extend_from_slice(&[0; 8]); // no width or height

    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&[0; 8]);
    mdhd.extend_from_slice(&AUDIO_RATE.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&0x55c4u16.to_be_bytes());
    mdhd.extend_from_slice(&[0; 2]);

    let mut hdlr = vec![0; 4];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0; 12]);
    hdlr.extend_from_slice(b"CTXRemote\0");

    // Opus in ISO BMFF: version, channels, pre-skip, input rate, gain, mapping.
    let mut dops = vec![0, 2];
    dops.extend_from_slice(&0u16.to_be_bytes());
    dops.extend_from_slice(&AUDIO_RATE.to_be_bytes());
    dops.extend_from_slice(&0i16.to_be_bytes());
    dops.push(0);

    let mut opus = vec![0; 6];
    opus.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    opus.extend_from_slice(&[0; 8]);
    opus.extend_from_slice(&2u16.to_be_bytes()); // channels
    opus.extend_from_slice(&16u16.to_be_bytes()); // sample size
    opus.extend_from_slice(&[0; 4]);
    opus.extend_from_slice(&(AUDIO_RATE << 16).to_be_bytes());
    opus.extend_from_slice(&boxed(b"dOps", &dops));

    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&boxed(b"Opus", &opus));
    let empty = 0u32.to_be_bytes();
    let stbl = boxed(
        b"stbl",
        &concat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &empty),
            full(b"stsc", 0, 0, &empty),
            full(b"stsz", 0, 0, &[0; 8]),
            full(b"stco", 0, 0, &empty),
        ]),
    );
    let mut dref = 1u32.to_be_bytes().to_vec();
    dref.extend_from_slice(&full(b"url ", 0, 1, &[]));
    let minf = boxed(
        b"minf",
        &concat(&[full(b"smhd", 0, 0, &[0; 4]), boxed(b"dinf", &full(b"dref", 0, 0, &dref)), stbl]),
    );
    let mdia = boxed(b"mdia", &concat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]));
    boxed(b"trak", &concat(&[full(b"tkhd", 0, 3, &tkhd), mdia]))
}

/// One fragment holding one sample; its data follows in an `mdat`.
fn moof(track: u32, sequence: u32, time: u64, duration: u32, size: u32, sync: bool) -> Vec<u8> {
    let mfhd = full(b"mfhd", 0, 0, &sequence.to_be_bytes());
    // Data offsets count from this moof.
    let tfhd = full(b"tfhd", 0, 0x02_0000, &track.to_be_bytes());
    let tfdt = full(b"tfdt", 1, 0, &time.to_be_bytes());
    // Sync samples depend on none; the others on earlier ones.
    let flags: u32 = if sync { 0x0200_0000 } else { 0x0101_0000 };
    let trun_len = 12 + 4 + 4 + 12;
    let moof_len = 8 + mfhd.len() + 8 + tfhd.len() + tfdt.len() + trun_len;
    let mut trun = 1u32.to_be_bytes().to_vec(); // sample count
    trun.extend_from_slice(&((moof_len + 8) as u32).to_be_bytes()); // to the mdat payload
    trun.extend_from_slice(&duration.to_be_bytes());
    trun.extend_from_slice(&size.to_be_bytes());
    trun.extend_from_slice(&flags.to_be_bytes());
    let trun = full(b"trun", 0, 0x0701, &trun);
    let traf = boxed(b"traf", &concat(&[tfhd, tfdt, trun]));
    let moof = boxed(b"moof", &concat(&[mfhd, traf]));
    debug_assert_eq!(moof.len(), moof_len);
    moof
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_round_trip() {
        let stream = [0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 1, 0x68, 4, 0, 0, 0, 1, 0x65, 9, 9, 0];
        let nals = split_annex_b(&stream);
        assert_eq!(nals, vec![&[0x67, 1, 2, 3][..], &[0x68, 4][..], &[0x65, 9, 9, 0][..]]);
        assert_eq!(to_avcc(&stream), vec![0, 0, 0, 4, 0x65, 9, 9, 0]);
    }

    /// Frames from the real encoder into a file; checked with ffprobe if it
    /// is installed (it is in CI images only sometimes, so not required).
    #[test]
    fn records_a_playable_file() {
        use crate::encoder::VideoEncoder;
        use ctxremote_proto::session::{Quality, VideoCodec};

        let (w, h) = (320u32, 240u32);
        let mut encoder = VideoEncoder::new(w, h, 30.0, Quality::Balanced).unwrap();
        let path = std::env::temp_dir().join(format!("ctxremote-record-{}.mp4", rand::random::<u32>()));
        let mut recorder: Option<Recorder> = None;
        for n in 0..45u32 {
            let bgra: Vec<u8> = (0..w * h).flat_map(|i| [(i + n * 4) as u8, (i / w) as u8, n as u8 * 5, 255]).collect();
            let encoded = encoder.encode(&bgra).unwrap();
            let frame = VideoFrame {
                display: 0,
                timestamp_us: n as u64 * 33_333,
                codec: VideoCodec::H264,
                keyframe: encoded.keyframe,
                width: w,
                height: h,
                data: encoded.data.to_vec(),
            };
            match &mut recorder {
                Some(r) => r.push(&frame).unwrap(),
                None => recorder = Some(Recorder::start(&path, &frame).unwrap()),
            }
        }
        let path = recorder.unwrap().finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        if let Ok(out) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
            .args(["-show_entries", "stream=codec_name,width,height,nb_read_frames", "-of", "csv=p=0"])
            .arg(&path)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            assert_eq!(text.trim(), "h264,320,240,45", "ffprobe: {text} {}", String::from_utf8_lossy(&out.stderr));
        }
        let _ = std::fs::remove_file(path);
    }

    /// Picture and sound from the real encoders; ffmpeg must see both
    /// streams and decode them without errors (when it is installed).
    #[test]
    fn records_sound_alongside() {
        use crate::encoder::VideoEncoder;
        use ctxremote_proto::session::{Quality, VideoCodec};

        let (w, h) = (320u32, 240u32);
        let mut encoder = VideoEncoder::new(w, h, 30.0, Quality::Balanced).unwrap();
        let mut opus = crate::audio::OpusFrames::new().unwrap();
        let tone: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let v = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
                [v, v]
            })
            .collect();
        let mut packets = Vec::new();
        opus.push(&tone, |p| packets.push(p));
        let path = std::env::temp_dir().join(format!("ctxremote-record-sound-{}.mp4", rand::random::<u32>()));
        let mut recorder: Option<Recorder> = None;
        let mut sound = packets.iter();
        for n in 0..30u32 {
            let bgra: Vec<u8> = (0..w * h).flat_map(|i| [(i + n) as u8, 0, 0, 255]).collect();
            let encoded = encoder.encode(&bgra).unwrap();
            let frame = VideoFrame {
                display: 0,
                timestamp_us: n as u64 * 33_333,
                codec: VideoCodec::H264,
                keyframe: encoded.keyframe,
                width: w,
                height: h,
                data: encoded.data.to_vec(),
            };
            match &mut recorder {
                Some(r) => r.push(&frame).unwrap(),
                None => recorder = Some(Recorder::start_with(&path, &frame, true).unwrap()),
            }
            // About one and a half packets per frame, as at 30 fps.
            for _ in 0..(1 + n % 2) {
                if let Some(p) = sound.next() {
                    recorder.as_mut().unwrap().push_audio(p).unwrap();
                }
            }
        }
        let path = recorder.unwrap().finish().unwrap();
        if let Ok(out) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
            .arg(&path)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            assert_eq!(text.split_whitespace().collect::<Vec<_>>(), vec!["h264", "opus"], "{text}");
            let decode = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-f", "null", "-"]).output().unwrap();
            assert!(decode.stderr.is_empty(), "{}", String::from_utf8_lossy(&decode.stderr));
        }
        let _ = std::fs::remove_file(path);
    }
}