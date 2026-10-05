//! Clipboard synchronisation between viewer and host: text and images both
//! ways, and on the host also files copied there (see `HostMsg::ClipboardFiles`).
//! Images travel as PNG.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use tracing::{debug, warn};

/// Larger texts are neither sent nor applied.
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// Larger images are neither sent nor applied (pixels; 8K × 4K).
pub const MAX_IMAGE_PIXELS: usize = 8192 * 4096;
/// An encoded image must fit a frame with room to spare.
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Without a change counter (not Windows), images are looked at only every
/// this many rounds: reading one is costly.
const IMAGE_ROUNDS: u32 = 5;

/// Remembers the last known clipboard text to detect local changes and avoid echoes.
#[derive(Default)]
struct Tracker {
    known: Option<String>,
}

impl Tracker {
    /// Returns the text to send if it is new, non-empty and small enough.
    fn observe(&mut self, current: Option<String>) -> Option<String> {
        if current == self.known {
            return None;
        }
        // Updated even for rejected texts, so they are not re-checked every round.
        self.known = current.clone();
        let text = current?;
        if text.is_empty() {
            return None;
        }
        if text.len() > MAX_CLIPBOARD_BYTES {
            debug!("Zwischenablage zu groß ({} Bytes), wird nicht übertragen", text.len());
            return None;
        }
        Some(text)
    }

    /// Records a text set by the peer so it is not sent back.
    fn applied(&mut self, text: &str) {
        self.known = Some(text.to_string());
    }
}

/// What the peer puts on this side's clipboard.
enum Apply {
    Text(String),
    Files(Vec<PathBuf>),
    /// PNG.
    Image(Vec<u8>),
}

/// Reports files copied here: once per new list, not lists set by the peer.
type OnFiles = Box<dyn Fn(Vec<PathBuf>) + Send>;
/// Reports images copied here, as PNG: once per new image, not the peer's.
pub type OnImage = Box<dyn Fn(Vec<u8>) + Send>;

/// Owns a thread that polls the local clipboard and applies the peer's text.
pub struct ClipboardSync {
    incoming: mpsc::Sender<Apply>,
}

impl ClipboardSync {
    /// Starts watching the clipboard. With `send_initial == false` the text present
    /// at start is not reported. Returns `None` if the clipboard is unavailable.
    pub fn start(send_initial: bool, on_change: impl Fn(String) + Send + 'static) -> Option<Self> {
        Self::start_with_files(send_initial, on_change, None, None)
    }

    /// Like [`ClipboardSync::start`], and `on_files` hears of files copied
    /// here, `on_image` of images.
    pub fn start_with_files(
        send_initial: bool,
        on_change: impl Fn(String) + Send + 'static,
        on_files: Option<OnFiles>,
        on_image: Option<OnImage>,
    ) -> Option<Self> {
        let (incoming, applying) = mpsc::channel::<Apply>();
        let (ready_tx, ready_rx) = mpsc::channel::<bool>();
        let spawned = std::thread::Builder::new().name("ctxremote-clipboard".into()).spawn(move || {
            // arboard is not Send everywhere, so the clipboard lives only on this thread.
            let mut clipboard = match arboard::Clipboard::new() {
                Ok(clipboard) => clipboard,
                Err(e) => {
                    warn!("Zwischenablage nicht verfügbar: {e}");
                    let _ = ready_tx.send(false);
                    return;
                }
            };
            let _ = ready_tx.send(true);

            let mut tracker = Tracker::default();
            let initial = tracker.observe(clipboard.get_text().ok());
            if send_initial {
                if let Some(text) = initial {
                    on_change(text);
                }
            }
            let mut sequence = sequence_number();
            // Files copied here already (or set by the peer), so each list is reported once.
            let mut known_files: Vec<PathBuf> = clipboard.get().file_list().unwrap_or_default();
            // Likewise the image present at start, by a hash of its pixels.
            let mut known_image: Option<u64> = on_image.as_ref().and_then(|_| clipboard.get_image().ok()).map(|i| image_hash(&i));
            let mut round = 0u32;
            loop {
                match applying.recv_timeout(POLL_INTERVAL) {
                    Ok(Apply::Text(text)) => match clipboard.set_text(text.clone()) {
                        Ok(()) => tracker.applied(&text),
                        Err(e) => warn!("Zwischenablage konnte nicht gesetzt werden: {e}"),
                    },
                    Ok(Apply::Files(files)) => match clipboard.set().file_list(&files) {
                        Ok(()) => known_files = files,
                        Err(e) => warn!("Dateien nicht in die Zwischenablage gelegt: {e}"),
                    },
                    Ok(Apply::Image(png)) => match decode_png(&png) {
                        Some(image) => {
                            let hash = image_hash(&image);
                            match clipboard.set_image(image) {
                                Ok(()) => known_image = Some(hash),
                                Err(e) => warn!("Bild nicht in die Zwischenablage gelegt: {e}"),
                            }
                        }
                        None => warn!("Bild der Gegenseite nicht lesbar"),
                    },
                    Err(RecvTimeoutError::Timeout) => {
                        // Opening the clipboard every round can make other apps' copy fail.
                        let current = sequence_number();
                        if current.is_some() && current == sequence {
                            continue;
                        }
                        sequence = current;
                        round = round.wrapping_add(1);
                        let text = clipboard.get_text().ok();
                        let mut files = Vec::new();
                        if let (None, Some(on_files)) = (&text, &on_files) {
                            files = clipboard.get().file_list().unwrap_or_default();
                            if !files.is_empty() && files != known_files {
                                on_files(files.clone());
                            }
                            known_files = files.clone();
                        }
                        let look = current.is_some() || round % IMAGE_ROUNDS == 0;
                        if let (true, None, true, Some(on_image)) = (look, &text, files.is_empty(), &on_image) {
                            let image = clipboard.get_image().ok();
                            let hash = image.as_ref().map(image_hash);
                            if hash != known_image {
                                known_image = hash;
                                if let Some(png) = image.as_ref().and_then(encode_png) {
                                    on_image(png);
                                }
                            }
                        }
                        if let Some(text) = tracker.observe(text) {
                            on_change(text);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        if spawned.is_err() {
            warn!("Zwischenablage-Thread konnte nicht gestartet werden");
            return None;
        }
        ready_rx.recv().unwrap_or(false).then_some(Self { incoming })
    }

    /// Sets the local clipboard to the peer's text.
    pub fn apply(&self, text: String) {
        if text.len() <= MAX_CLIPBOARD_BYTES {
            let _ = self.incoming.send(Apply::Text(text));
        }
    }

    /// Puts files on the local clipboard, as if copied in the file manager.
    pub fn apply_files(&self, files: Vec<PathBuf>) {
        let _ = self.incoming.send(Apply::Files(files));
    }

    /// Sets the local clipboard to the peer's image (PNG).
    pub fn apply_image(&self, png: Vec<u8>) {
        if png.len() <= MAX_IMAGE_BYTES {
            let _ = self.incoming.send(Apply::Image(png));
        }
    }
}

fn image_hash(image: &arboard::ImageData) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (image.width, image.height).hash(&mut hasher);
    image.bytes.hash(&mut hasher);
    hasher.finish()
}

/// The image as PNG, if it is within the limits.
fn encode_png(image: &arboard::ImageData) -> Option<Vec<u8>> {
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 || w * h > MAX_IMAGE_PIXELS || image.bytes.len() != w * h * 4 {
        debug!("Bild in der Zwischenablage zu groß oder leer ({w}×{h}), wird nicht übertragen");
        return None;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // Screenshots compress well already; the fast setting keeps copying snappy.
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&image.bytes).ok()?;
    writer.finish().ok()?;
    (out.len() <= MAX_IMAGE_BYTES).then_some(out)
}

/// Decodes a PNG to RGBA, refusing what is beyond the limits.
fn decode_png(data: &[u8]) -> Option<arboard::ImageData<'static>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    if w == 0 || h == 0 || w * h > MAX_IMAGE_PIXELS {
        return None;
    }
    let mut buffer = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(frame.buffer_size());
    let rgba = match (frame.color_type, frame.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buffer,
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => {
            buffer.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        _ => return None,
    };
    (rgba.len() == w * h * 4).then(|| arboard::ImageData { width: w, height: h, bytes: rgba.into() })
}

/// The files on this computer's clipboard, if it holds files.
pub fn local_files() -> Vec<PathBuf> {
    arboard::Clipboard::new().and_then(|mut c| c.get().file_list()).unwrap_or_default()
}

/// Puts files on this computer's clipboard.
pub fn set_local_files(files: &[PathBuf]) -> anyhow::Result<()> {
    arboard::Clipboard::new()?.set().file_list(files)?;
    Ok(())
}

/// Changes with every clipboard update; `None` where the platform has no such counter.
#[cfg(windows)]
fn sequence_number() -> Option<u32> {
    Some(unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() })
}

#[cfg(not(windows))]
fn sequence_number() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn reports_new_text_once() {
        let mut t = Tracker::default();
        assert_eq!(t.observe(some("a")), some("a"));
        assert_eq!(t.observe(some("a")), None);
        assert_eq!(t.observe(some("b")), some("b"));
    }

    #[test]
    fn ignores_empty_and_missing() {
        let mut t = Tracker::default();
        assert_eq!(t.observe(None), None);
        assert_eq!(t.observe(some("")), None);
    }

    #[test]
    fn applied_text_is_not_echoed() {
        let mut t = Tracker::default();
        t.applied("from peer");
        assert_eq!(t.observe(some("from peer")), None);
        assert_eq!(t.observe(some("local")), some("local"));
    }

    #[test]
    fn png_round_trip() {
        let bytes: Vec<u8> = (0..3 * 2 * 4).map(|i| (i * 11) as u8).collect();
        let image = arboard::ImageData { width: 3, height: 2, bytes: bytes.clone().into() };
        let png = encode_png(&image).unwrap();
        let back = decode_png(&png).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(back.bytes.as_ref(), bytes.as_slice());
        assert_eq!(image_hash(&image), image_hash(&back));
        assert!(decode_png(b"kein png").is_none());
        let empty = arboard::ImageData { width: 0, height: 0, bytes: Vec::new().into() };
        assert!(encode_png(&empty).is_none());
    }

    #[test]
    fn oversized_text_is_not_reported() {
        let mut t = Tracker::default();
        let big = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert_eq!(t.observe(Some(big.clone())), None);
        assert_eq!(t.observe(Some(big)), None);
        assert_eq!(t.observe(some("ok")), some("ok"));
    }
}
