//! End-to-end encrypted messages between a viewer and a host.
//!
//! Compatibility: postcard cannot skip unknown enum variants, so a message an
//! older peer does not know ends its session. New variants are therefore only
//! sent to peers that announce the matching [`Features`] bit. The features
//! travel as a trailer after `Hello` and `Welcome`, which older versions
//! ignore (postcard does not read past the message).

use serde::{Deserialize, Serialize};

use crate::DeviceId;

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
    /// Sound of the host, after the viewer asked with `SetAudio(true)`.
    Audio(AudioPacket),
    /// What the person at the host allows in this session; sent at the start
    /// and on every change, only to viewers with [`Features::RIGHTS`].
    Rights(Permissions),
    /// Whether privacy mode is on, after [`ViewerMsg::Privacy`] or when the
    /// host ended it; `error` says why it could not be turned on.
    Privacy { on: bool, error: Option<String> },
    /// Before `Welcome`: the host wants the code from its authenticator app
    /// (two-factor for the permanent password). Answered with
    /// [`ViewerMsg::Code`]; only to viewers with [`Features::CODE`].
    CodeRequired,
    /// Files were copied at the host: their paths, for the viewer to fetch
    /// if the user wants them; only to viewers with [`Features::FILE_PASTE`].
    ClipboardFiles(Vec<String>),
    /// The answer to [`ViewerMsg::GetSystemInfo`].
    SystemInfo(SystemInfo),
    /// A port tunnel's traffic (host → viewer); see [`TunnelMsg`].
    Tunnel(TunnelMsg),
    /// An image was copied at the host, as PNG; only to viewers with
    /// [`Features::CLIPBOARD_IMAGE`].
    ClipboardImage(Vec<u8>),
}

/// Port tunnels: TCP connections the viewer accepts locally and the host
/// opens to a target in its network, carried inside the session. Each side
/// keeps at most a window of unacknowledged bytes per connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelMsg {
    /// Viewer → host: connect to `target` ("host:port") for connection `id`.
    Open { id: u32, target: String },
    /// Host → viewer: the connection stands, or why not.
    Opened { id: u32, result: Result<(), String> },
    Data { id: u32, data: Vec<u8> },
    /// The receiver passed on `bytes` more; the sender may send that much again.
    Ack { id: u32, bytes: u32 },
    /// The sender's side ended (or failed); no more data follows.
    Close { id: u32 },
}

/// What the host's computer is, for the viewer's info panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub hostname: String,
    /// The user signed in at the computer, if any.
    pub user: String,
    /// E.g. "Windows 11 Pro 24H2".
    pub os: String,
    /// Build or kernel version.
    pub os_build: String,
    /// Maker and model, where the system tells.
    pub model: String,
    pub cpu: String,
    pub cores: u32,
    pub memory_total: u64,
    pub memory_used: u64,
    pub uptime_secs: u64,
    pub disks: Vec<DiskInfo>,
    pub networks: Vec<NetworkInfo>,
    /// The CTXRemote version on the host.
    pub app_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Drive or mount point, e.g. `C:\`.
    pub mount: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub name: String,
    /// `aa:bb:cc:dd:ee:ff`; used to wake the computer over the network.
    pub mac: String,
    pub addresses: Vec<String>,
}

/// What a viewer may do in a session, as bits. The host enforces them;
/// the viewer only hides what it may not use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permissions(pub u32);

impl Permissions {
    /// Mouse and keyboard, including Ctrl+Alt+Del and locking the screen.
    pub const INPUT: u32 = 1 << 0;
    pub const FILES: u32 = 1 << 1;
    pub const CLIPBOARD: u32 = 1 << 2;
    pub const AUDIO: u32 = 1 << 3;
    pub const RESTART: u32 = 1 << 4;
    /// Blank the host's screen and block its local input.
    pub const PRIVACY: u32 = 1 << 5;
    /// Port tunnels into the host's network.
    pub const TUNNEL: u32 = 1 << 6;

    pub const ALL: Self = Self(
        Self::INPUT | Self::FILES | Self::CLIPBOARD | Self::AUDIO | Self::RESTART | Self::PRIVACY | Self::TUNNEL,
    );
    /// For someone sitting at the host: no blanking their screen, no way
    /// into their network.
    pub const ATTENDED: Self = Self(Self::ALL.0 & !Self::PRIVACY & !Self::TUNNEL);
    pub const VIEW_ONLY: Self = Self(0);

    pub fn has(self, right: u32) -> bool {
        self.0 & right == right
    }

    pub fn with(self, right: u32, on: bool) -> Self {
        if on { Self(self.0 | right) } else { Self(self.0 & !right) }
    }
}

/// 20 ms of the host's sound: one Opus packet, 48 kHz stereo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioPacket {
    pub data: Vec<u8>,
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
    /// Reads a [`MemberProof`] in the `Hello` trailer and offers devices of
    /// its account access without a password, if the user allowed that.
    pub const ACCOUNT: u32 = 1 << 8;
    /// Understands `SetAudio` and sends `Audio`.
    pub const AUDIO: u32 = 1 << 9;
    /// Understands `Rights` and enforces permissions (the host always does).
    pub const RIGHTS: u32 = 1 << 10;
    /// Understands `ViewerMsg::Privacy` and `HostMsg::Privacy`.
    pub const PRIVACY: u32 = 1 << 11;
    /// Answers `CodeRequired` with `Code` (two-factor).
    pub const CODE: u32 = 1 << 12;
    /// Files through the clipboard: `PasteDir`, `ClipboardFromDir`, `ClipboardFiles`.
    pub const FILE_PASTE: u32 = 1 << 13;
    /// Answers `GetSystemInfo`.
    pub const SYSINFO: u32 = 1 << 14;
    /// Shows `Recording` to the person at the host.
    pub const RECORDING: u32 = 1 << 15;
    /// Carries port tunnels (`Tunnel`).
    pub const TUNNEL: u32 = 1 << 16;
    /// Shows `Draw` lines over its screen.
    pub const DRAW: u32 = 1 << 17;
    /// Plays `Mic` packets.
    pub const MIC: u32 = 1 << 18;
    /// Takes and sends `ClipboardImage`.
    pub const CLIPBOARD_IMAGE: u32 = 1 << 19;
    /// Types `InputEvent::Text`.
    pub const TYPE_TEXT: u32 = 1 << 20;
    /// Continues interrupted single-file transfers (`DownloadFrom`, `UploadFrom`).
    pub const RESUME: u32 = 1 << 21;

    /// Everything this build supports.
    pub const CURRENT: Self = Self(
        Self::FILES
            | Self::CURSOR
            | Self::RESTART
            | Self::QUALITY
            | Self::DIRECT
            | Self::CHAT
            | Self::PUNCH
            | Self::PROFILE
            | Self::ACCOUNT
            | Self::AUDIO
            | Self::RIGHTS
            | Self::PRIVACY
            | Self::CODE
            | Self::FILE_PASTE
            | Self::SYSINFO
            | Self::RECORDING
            | Self::TUNNEL
            | Self::DRAW
            | Self::MIC
            | Self::CLIPBOARD_IMAGE
            | Self::TYPE_TEXT
            | Self::RESUME,
    );
    /// What a peer without a trailer (an older version) understands.
    pub const NONE: Self = Self(0);

    pub fn has(self, feature: u32) -> bool {
        self.0 & feature == feature
    }
}

/// What follows `Hello` in the same frame. `features` comes first, so hosts
/// that only know the plain [`Features`] trailer still read it; later fields
/// are optional at the end, so each version reads what it knows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HelloExtras {
    pub features: Features,
    pub profile: Option<HelperProfile>,
    pub member: Option<MemberProof>,
}

impl HelloExtras {
    /// Reads whatever trailer the viewer sent: extras of any version, plain
    /// features (older viewers) or nothing (oldest viewers).
    pub fn decode(trailer: &[u8]) -> Self {
        let Ok((features, rest)) = postcard::take_from_bytes::<Features>(trailer) else {
            return Self::default();
        };
        let (profile, rest) = postcard::take_from_bytes::<Option<HelperProfile>>(rest).unwrap_or((None, &[]));
        let member = postcard::take_from_bytes::<Option<MemberProof>>(rest).map_or(None, |(m, _)| m);
        Self { features, profile, member }
    }
}

/// A viewer's claim to be a device of an account: its device key and a
/// signature over the host's ID and this session's binding value (see
/// [`crate::secure::SecureReceiver::binding`]), so it cannot be replayed in
/// another session. The host still asks the server whether the key is a
/// current member, and the password slot proved the account key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProof {
    pub public_key: [u8; 32],
    pub signature: Vec<u8>,
}

fn member_payload(host: DeviceId, binding: &[u8; 32]) -> Vec<u8> {
    [b"ctxremote/member/v1:".as_slice(), &host.get().to_be_bytes(), binding].concat()
}

impl MemberProof {
    pub fn sign(key: &ed25519_dalek::SigningKey, host: DeviceId, binding: &[u8; 32]) -> Self {
        use ed25519_dalek::Signer;
        Self {
            public_key: key.verifying_key().to_bytes(),
            signature: key.sign(&member_payload(host, binding)).to_bytes().to_vec(),
        }
    }

    pub fn verify(&self, host: DeviceId, binding: &[u8; 32]) -> bool {
        use ed25519_dalek::Verifier;
        let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&self.public_key) else { return false };
        let Ok(signature) = ed25519_dalek::Signature::from_slice(&self.signature) else { return false };
        key.verify(&member_payload(host, binding), &signature).is_ok()
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
    /// Turns the host's sound on or off; only to hosts with [`Features::AUDIO`].
    SetAudio(bool),
    /// Turns privacy mode on or off; only to hosts with [`Features::PRIVACY`].
    Privacy(bool),
    /// The answer to [`HostMsg::CodeRequired`].
    Code(String),
    /// Asks for [`HostMsg::SystemInfo`]; only to hosts with [`Features::SYSINFO`].
    GetSystemInfo,
    /// The viewer records the session (or stopped); the host shows it to the
    /// person there. Only to hosts with [`Features::RECORDING`].
    Recording(bool),
    /// A port tunnel's traffic (viewer → host); only to hosts with [`Features::TUNNEL`].
    Tunnel(TunnelMsg),
    /// Drawing over the host's screen; only to hosts with [`Features::DRAW`].
    Draw(DrawMsg),
    /// The viewer's microphone, one Opus packet (20 ms, 48 kHz mono), played
    /// at the host; only to hosts with [`Features::MIC`].
    Mic(AudioPacket),
    /// An image for the host's clipboard, as PNG; only to hosts with
    /// [`Features::CLIPBOARD_IMAGE`].
    ClipboardImage(Vec<u8>),
}

/// Lines the viewer draws over the shown display, visible to the person at
/// the host and in the picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawMsg {
    /// A line through `points`, each 0..=65535 across the display's width
    /// and height; `color` as 0xRRGGBB.
    Stroke { color: u32, width: u8, points: Vec<(u16, u16)> },
    /// Removes all lines (also when the viewer leaves drawing).
    Clear,
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
    /// A new, empty folder for files to paste (answered with `Path`); only to
    /// hosts with [`Features::FILE_PASTE`].
    PasteDir,
    /// Puts everything in `dir` (from `PasteDir`) on the host's clipboard as
    /// files, ready for Ctrl+V there.
    ClipboardFromDir { dir: String },
    /// Like `Download`, continuing a single file of `size` bytes at `offset`,
    /// where the viewer's interrupted copy ends. Answered with `Offset`: where
    /// the host continues (0 if the file changed). Only to hosts with
    /// [`Features::RESUME`].
    DownloadFrom { id: u32, path: String, offset: u64, size: u64 },
    /// Like `Upload` for a single file `name` of `size` bytes; the host answers
    /// with `Offset`, the bytes of an interrupted copy it already has, and the
    /// viewer sends the rest. Only to hosts with [`Features::RESUME`].
    UploadFrom { id: u32, dir: String, name: String, size: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileReply {
    Listing(Listing),
    Done,
    /// A folder on the host, after `PasteDir`.
    Path(String),
    /// Where a continued transfer starts, after `DownloadFrom` or `UploadFrom`.
    Offset(u64),
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
    /// Text typed character by character, whatever the host's keyboard
    /// layout (e.g. a password from the clipboard on the sign-in screen);
    /// only to hosts with [`Features::TYPE_TEXT`].
    Text(String),
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
        let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let member = MemberProof::sign(&key, DeviceId::new(123_456_789).unwrap(), &[9; 32]);
        let extras = HelloExtras { features: Features::CURRENT, profile: Some(profile.clone()), member: Some(member.clone()) };

        // An older host reads only the features from the new trailer.
        let frame = hello_frame(Some(&extras));
        let (_, rest) = postcard::take_from_bytes::<ViewerMsg>(&frame).unwrap();
        assert_eq!(postcard::from_bytes::<Features>(rest).unwrap(), Features::CURRENT);
        let decoded = HelloExtras::decode(rest);
        assert_eq!((decoded.features, decoded.profile.clone(), decoded.member), (Features::CURRENT, Some(profile.clone()), Some(member)));

        // A host from before member proofs still reads features and profile.
        #[derive(Deserialize)]
        struct ProfileOnly {
            features: Features,
            profile: Option<HelperProfile>,
        }
        let old = postcard::from_bytes::<ProfileOnly>(rest).unwrap();
        assert_eq!((old.features, old.profile), (Features::CURRENT, Some(profile.clone())));

        // A viewer from before member proofs: profile without a member.
        #[derive(Serialize)]
        struct OldExtras {
            features: Features,
            profile: Option<HelperProfile>,
        }
        let frame = hello_frame(Some(&OldExtras { features: Features(Features::PROFILE), profile: Some(profile.clone()) }));
        let (_, rest) = postcard::take_from_bytes::<ViewerMsg>(&frame).unwrap();
        let decoded = HelloExtras::decode(rest);
        assert_eq!((decoded.profile, decoded.member), (Some(profile), None));

        // A newer host reads older viewers' trailers.
        let frame = hello_frame(Some(&Features(Features::FILES)));
        let (_, rest) = postcard::take_from_bytes::<ViewerMsg>(&frame).unwrap();
        let decoded = HelloExtras::decode(rest);
        assert_eq!((decoded.features, decoded.profile), (Features(Features::FILES), None));
        assert_eq!(HelloExtras::decode(&[]).features, Features::NONE);
    }

    #[test]
    fn member_proofs_bind_host_and_session() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let host = DeviceId::new(123_456_789).unwrap();
        let proof = MemberProof::sign(&key, host, &[9; 32]);
        assert!(proof.verify(host, &[9; 32]));
        assert!(!proof.verify(host, &[8; 32]), "another session");
        assert!(!proof.verify(DeviceId::new(987_654_321).unwrap(), &[9; 32]), "another host");
        let forged = MemberProof { public_key: [7; 32], ..proof };
        assert!(!forged.verify(host, &[9; 32]));
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
