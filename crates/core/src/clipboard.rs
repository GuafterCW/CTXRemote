//! Clipboard synchronisation between viewer and host: text both ways, and on
//! the host also files copied there (see `HostMsg::ClipboardFiles`).

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use tracing::{debug, warn};

/// Larger texts are neither sent nor applied.
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(300);

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
}

/// Reports files copied here: once per new list, not lists set by the peer.
type OnFiles = Box<dyn Fn(Vec<PathBuf>) + Send>;

/// Owns a thread that polls the local clipboard and applies the peer's text.
pub struct ClipboardSync {
    incoming: mpsc::Sender<Apply>,
}

impl ClipboardSync {
    /// Starts watching the clipboard. With `send_initial == false` the text present
    /// at start is not reported. Returns `None` if the clipboard is unavailable.
    pub fn start(send_initial: bool, on_change: impl Fn(String) + Send + 'static) -> Option<Self> {
        Self::start_with_files(send_initial, on_change, None)
    }

    /// Like [`ClipboardSync::start`], and `on_files` hears of files copied here.
    pub fn start_with_files(
        send_initial: bool,
        on_change: impl Fn(String) + Send + 'static,
        on_files: Option<OnFiles>,
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
                    Err(RecvTimeoutError::Timeout) => {
                        // Opening the clipboard every round can make other apps' copy fail.
                        let current = sequence_number();
                        if current.is_some() && current == sequence {
                            continue;
                        }
                        sequence = current;
                        let text = clipboard.get_text().ok();
                        if let (None, Some(on_files)) = (&text, &on_files) {
                            let files = clipboard.get().file_list().unwrap_or_default();
                            if !files.is_empty() && files != known_files {
                                on_files(files.clone());
                            }
                            known_files = files;
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
    fn oversized_text_is_not_reported() {
        let mut t = Tracker::default();
        let big = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert_eq!(t.observe(Some(big.clone())), None);
        assert_eq!(t.observe(Some(big)), None);
        assert_eq!(t.observe(some("ok")), some("ok"));
    }
}
