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
    /// A UDP path through NAT for the token of the earlier `DirectOffer`:
    /// the host's UDP `candidates` and the SHA-256 of its QUIC certificate.
    /// The viewer answers with [`ViewerMsg::PunchAnswer`].
    PunchOffer { candidates: Vec<String>, cert: [u8; 32] },
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
    /// `PunchOffer` / `PunchAnswer`: direct connections through NAT.
    pub const PUNCH: u32 = 1 << 6;
    /// Reads a [`HelperProfile`] in the `Hello` trailer. Informational only:
    /// viewers send the profile either way, older hosts skip it.
    pub const PROFILE: u32 = 1 << 7;

    /// Everything this build supports.
    pub const CURRENT: Self = Self(Self::FILES | Self::CURSOR | Self::RESTART | Self::QUALITY | Self::DIRECT | Self::CHAT | Self::PUNCH | Self::PROFILE);
    /// What a peer without a trailer (an older version) understands.
    pub const NONE: Self = Self(0);

    pub fn has(self, feature: u32) -> bool {
        self.0 & feature == feature
    }
}

/// What follows `Hello` in the same frame. `features` comes first, so hosts
/// that only know the plain [`Features`] trailer still read it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HelloExtras {
    pub features: Features,
    pub profile: Option<HelperProfile>,
}

impl HelloExtras {
    /// Reads whatever trailer the viewer sent: extras, plain features (older
    /// viewers) or nothing (oldest viewers).
    pub fn decode(trailer: &[u8]) -> Self {
        if trailer.is_empty() {
            return Self::default();
        }
        if let Ok(extras) = postcard::from_bytes::<Self>(trailer) {
            return extras;
        }
        Self { features: postcard::from_bytes(trailer).unwrap_or_default(), profile: None }
    }
}

/// How the person at the viewer presents themselves to the person at the
/// host, e.g. in the quick helper's consent dialog. Self-declared, so the
/// host shows it as such. Limits: see the `MAX_*` constants; hosts cut longer
/// texts and drop a logo that is too big or not a PNG.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperProfile {
    pub name: String,
    pub company: String,
    /// A short note, e.g. "Ich helfe Ihnen beim Drucker".
    pub message: String,
    /// PNG, at most [`HelperProfile::MAX_LOGO`] bytes; empty for none.
    pub logo: Vec<u8>,
}

impl HelperProfile {
    pub const MAX_NAME: usize = 60;
    pub const MAX_COMPANY: usize = 80;
    pub const MAX_MESSAGE: usize = 300;
    pub const MAX_LOGO: usize = 64 * 1024;
    const PNG: &'static [u8] = b"\x89PNG\r\n\x1a\n";

    /// Trims and cuts every field to its limit and drops an invalid logo;
    /// `None` if nothing is left.
    pub fn sanitized(self) -> Option<Self> {
        let logo = if self.logo.len() <= Self::MAX_LOGO && self.logo.starts_with(Self::PNG) { self.logo } else { Vec::new() };
        let profile = Self {
            name: clean(&self.name, Self::MAX_NAME),
            company: clean(&self.company, Self::MAX_COMPANY),
            message: clean(&self.message, Self::MAX_MESSAGE),
            logo,
        };
        (!profile.name.is_empty() || !profile.company.is_empty()).then_some(profile)
    }
}

/// Trimmed, without control characters (line breaks become spaces), at most `max` characters.
fn clean(text: &str, max: usize) -> String {
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    /// The viewer's UDP candidates; both sides then punch towards each other.
    PunchAnswer { candidates: Vec<String> },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_frame<U: Serialize>(trailer: Option<&U>) -> Vec<u8> {
        let mut frame = postcard::to_stdvec(&ViewerMsg::Hello { name: "a".into(), device: None }).unwrap();
        if let Some(trailer) = trailer {
            frame.extend(postcard::to_stdvec(trailer).unwrap());
        }
        frame
    }

    #[test]
    fn hello_extras_are_compatible_both_ways() {
        let profile = HelperProfile { name: "Philipp".into(), company: "Ecker IT".into(), message: String::new(), logo: vec![] };
        let extras = HelloExtras { features: Features::CURRENT, profile: Some(profile.clone()) };

        // An older host reads only the features from the new trailer.
        let frame = hello_frame(Some(&extras));
        let (_, rest) = postcard::take_from_bytes::<ViewerMsg>(&frame).unwrap();
        assert_eq!(postcard::from_bytes::<Features>(rest).unwrap(), Features::CURRENT);
        let decoded = HelloExtras::decode(rest);
        assert_eq!((decoded.features, decoded.profile), (Features::CURRENT, Some(profile)));

        // A newer host reads older viewers' trailers.
        let frame = hello_frame(Some(&Features(Features::FILES)));
        let (_, rest) = postcard::take_from_bytes::<ViewerMsg>(&frame).unwrap();
        let decoded = HelloExtras::decode(rest);
        assert_eq!((decoded.features, decoded.profile), (Features(Features::FILES), None));
        assert_eq!(HelloExtras::decode(&[]).features, Features::NONE);
    }

    #[test]
    fn profiles_are_cut_to_size() {
        let long = HelperProfile {
            name: format!("  Max\n{}", "x".repeat(200)),
            company: "Firma\u{7}".into(),
            message: "Hallo".into(),
            logo: vec![1, 2, 3],
        };
        let clean = long.sanitized().unwrap();
        assert!(clean.name.starts_with("Max x") && clean.name.chars().count() <= HelperProfile::MAX_NAME);
        assert_eq!(clean.company, "Firma");
        assert!(clean.logo.is_empty(), "not a PNG");

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(100, 0);
        let with_logo = HelperProfile { name: "A".into(), logo: png.clone(), ..Default::default() };
        assert_eq!(with_logo.sanitized().unwrap().logo, png);
        png.resize(HelperProfile::MAX_LOGO + 1, 0);
        assert!(HelperProfile { name: "A".into(), logo: png, ..Default::default() }.sanitized().unwrap().logo.is_empty());
        assert!(HelperProfile { message: "nur Text".into(), ..Default::default() }.sanitized().is_none());
    }
}
