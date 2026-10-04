//! End-to-end encrypted messages between a viewer and a host.
//!
//! Compatibility: postcard cannot skip unknown enum variants, so a message an
//! older peer does not know ends its session. New variants are therefore only
//! sent to peers that announce the matching [`Features`] bit. The features
//! travel as a trailer after `Hello` and `Welcome`, which older versions
//! ignore (postcard does not read past the message).

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
    /// The host's mouse pointer changed shape; the viewer shows it over the image.
    Cursor(CursorShape),
    /// A direct route to the host: the viewer may connect to any of `addrs`
    /// and present `token` in a [`DirectHello`] (see `docs/DIRECT.md`).
    DirectOffer { addrs: Vec<String>, token: [u8; 32] },
    /// Last message on the old route; everything after it comes over the direct one.
    Switch,
    /// A chat message from the person at the host.
    Chat(String),
}

/// Longest chat message in bytes; longer ones are cut by the sender.
pub const MAX_CHAT: usize = 4000;

/// What a peer understands beyond the first protocol version, as bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features(pub u32);

impl Features {
    pub const FILES: u32 = 1 << 0;
    pub const CURSOR: u32 = 1 << 1;
    pub const RESTART: u32 = 1 << 2;
    pub const QUALITY: u32 = 1 << 3;
    pub const DIRECT: u32 = 1 << 4;
    pub const CHAT: u32 = 1 << 5;

    /// Everything this build supports.
    pub const CURRENT: Self = Self(Self::FILES | Self::CURSOR | Self::RESTART | Self::QUALITY | Self::DIRECT | Self::CHAT);
    /// What a peer without a trailer (an older version) understands.
    pub const NONE: Self = Self(0);

    pub fn has(self, feature: u32) -> bool {
        self.0 & feature == feature
    }
}

/// The only plaintext frame on a direct connection, sent by the viewer before
/// the session's encrypted frames continue there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectHello {
    pub token: [u8; 32],
}

/// A mouse pointer image, straight (not premultiplied) RGBA, row by row.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorShape {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for CursorShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorShape")
            .field("size", &(self.width, self.height))
            .field("hotspot", &(self.hot_x, self.hot_y))
            .finish()
    }
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
    /// Restarts the host computer; the host ends the session with `Bye`.
    Restart,
    /// Trades image quality against bandwidth for the rest of the session.
    SetQuality(Quality),
    /// Last message on the old route; everything after it goes over the direct one.
    Switch,
    /// A chat message from the person at the viewer.
    Chat(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Quality {
    /// Fewer bits for slow links.
    Speed,
    #[default]
    Balanced,
    /// More bits for crisp text on fast links.
    Sharp,
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
