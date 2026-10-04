//! End-to-end encrypted messages between a viewer and a host.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    /// Always the first message, answering the viewer's `Hello`.
    Welcome(HostInfo),
    Video(VideoFrame),
    Clipboard(String),
    /// The host ends the session, with a reason for the viewer.
    Bye(String),
    /// Answer to [`ViewerMsg::File`] with the same request number.
    FileReply { req: u32, result: Result<FileReply, String> },
    /// Part of a download (host → viewer), see [`Transfer`].
    Transfer { id: u32, msg: Transfer },
    /// Bytes of an upload written so far; the viewer keeps only a window unacknowledged.
    TransferAck { id: u32, bytes: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerMsg {
    /// Always the first message: who is connecting, shown on the host.
    Hello { name: String, device: Option<crate::DeviceId> },
    Input(InputEvent),
    SelectDisplay(u8),
    RequestKeyframe,
    Clipboard(String),
    Bye,
    /// Triggers Ctrl+Alt+Del on the host; needs the Windows service.
    SecureAttention,
    /// Locks the host's session (injected Win+L is ignored by Windows).
    LockScreen,
    /// A file browser request; answered by [`HostMsg::FileReply`].
    File { req: u32, op: FileOp },
    /// Part of an upload (viewer → host), see [`Transfer`].
    Transfer { id: u32, msg: Transfer },
}

/// File operations on the host. Paths are absolute and in the host's own syntax;
/// the empty path stands for the top level (drives and the user's folders).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileOp {
    List { path: String },
    CreateDir { path: String },
    /// Renames within the same folder; `name` is the new name only.
    Rename { path: String, name: String },
    /// Deletes files and folders including their content.
    Delete { paths: Vec<String> },
    /// Starts download `id` of a file or folder; the host answers with `Done`
    /// and then sends the content as [`HostMsg::Transfer`] messages.
    Download { id: u32, path: String },
    /// Announces upload `id` into the folder `dir`; the content follows as
    /// [`ViewerMsg::Transfer`] messages.
    Upload { id: u32, dir: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileReply {
    Listing(Listing),
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listing {
    /// The listed folder, normalised; empty for the top level.
    pub path: String,
    /// `None` at the top level.
    pub parent: Option<String>,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    /// Absolute path, ready to be listed or transferred.
    pub path: String,
    pub kind: EntryKind,
    /// Bytes; 0 for folders.
    pub size: u64,
    /// Unix seconds, 0 if unknown.
    pub modified: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Dir,
    /// A drive or volume root.
    Drive,
    /// A well-known folder of the user (Desktop, Documents, Downloads).
    Place,
}

/// The content of one transfer, the same in both directions. One transfer
/// carries one file or one folder tree: entries with relative paths (`/` as
/// separator, never `..` or absolute), each `File` followed by its data.
/// The receiver picks a free name for the top-level item instead of overwriting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Transfer {
    /// Always first: the total number of content bytes that will follow.
    Start { total: u64 },
    Dir { rel: String },
    File { rel: String, size: u64 },
    Data(Vec<u8>),
    /// Everything was sent.
    End,
    /// The sender gave up; the receiver discards what it got.
    Failed(String),
    /// The receiver (or the user on either side) aborts; the sender stops.
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub displays: Vec<DisplayInfo>,
    pub active_display: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub index: u8,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    /// H.264 Annex B, constrained baseline.
    H264,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct VideoFrame {
    pub display: u8,
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub codec: VideoCodec,
    /// Microseconds since the stream started.
    pub timestamp_us: u64,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("display", &self.display)
            .field("size", &(self.width, self.height))
            .field("keyframe", &self.keyframe)
            .field("bytes", &self.data.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    /// Pixel position within the active display.
    MouseMove { x: i32, y: i32 },
    MouseButton { button: MouseButton, down: bool },
    /// Wheel deltas in Windows units, 120 per notch.
    Wheel { dx: i32, dy: i32 },
    /// A physical key, identified by its DOM `KeyboardEvent.code`.
    Key { code: String, down: bool },
    /// Releases every key and button the viewer may still hold, e.g. on focus loss.
    ReleaseAll,
}
