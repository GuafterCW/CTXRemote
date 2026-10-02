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
