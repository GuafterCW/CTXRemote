#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(feature = "quick"))]
mod account;
#[cfg(feature = "quick")]
mod quick;
#[cfg(not(feature = "quick"))]
mod service;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use ctxremote_core::config::{Config, DirectSettings};
use ctxremote_core::files::client::{FileClient, TransferEvent};
use ctxremote_core::files::{self, UserContext};
use ctxremote_core::host::{Host, Presence};
use ctxremote_core::profile::Profile;
use ctxremote_core::proto::session::{CursorShape, HostInfo, InputEvent, Listing, Permissions, Quality, VideoFrame, ViewerMsg};
use ctxremote_core::proto::DeviceId;
#[cfg(not(feature = "quick"))]
use ctxremote_core::ui_link::UiRequest;
use ctxremote_core::viewer::{ViewerEvent, ViewerSession};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
#[cfg(not(feature = "quick"))]
use tauri::menu::{Menu, MenuItem};
#[cfg(not(feature = "quick"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

struct AppState {
    config: Arc<RwLock<Config>>,
    host: Side,
    viewers: Mutex<HashMap<u32, Viewer>>,
    next_viewer: AtomicU32,
    /// Local paths dropped on a session window, waiting for its file window.
    drops: Mutex<HashMap<u32, Vec<String>>>,
    /// A newer signed release on the server (app mode only; the service updates itself).
    update: Mutex<Option<ctxremote_core::proto::update::UpdateInfo>>,
}

/// Who hosts this device: the app itself, or the installed Windows service.
enum Side {
    Local(Host),
    #[cfg(not(feature = "quick"))]
    Service(Arc<service::Service>),
}

impl Side {
    fn end_session(&self, session: u64) {
        match self {
            Side::Local(host) => host.end_session(session),
            #[cfg(not(feature = "quick"))]
            Side::Service(service) => service.send(UiRequest::EndSession(session)),
        }
    }

    fn set_rights(&self, session: u64, rights: Permissions) -> Result<(), String> {
        match self {
            Side::Local(host) => host.set_rights(session, rights),
            #[cfg(not(feature = "quick"))]
            Side::Service(service) => {
                service.send(UiRequest::SetRights { session, rights });
                Ok(())
            }
        }
    }

    fn send_chat(&self, session: u64, text: String) -> Result<(), String> {
        match self {
            Side::Local(host) => host.send_chat(session, &text),
            #[cfg(not(feature = "quick"))]
            Side::Service(service) => {
                service.send(UiRequest::Chat { session, text });
                Ok(())
            }
        }
    }
}

struct Viewer {
    session: ViewerSession,
    target: DeviceId,
    link: Arc<Mutex<Link>>,
}

/// Where a session's video goes. Frames that arrive before the window has
/// attached are dropped; the window asks for a fresh keyframe on attach.
#[derive(Default)]
struct Link {
    channel: Option<Channel<InvokeResponseBody>>,
    closed: Option<Option<String>>,
    /// Keeps the clipboard watcher alive for the session's lifetime.
    clipboard: Option<ctxremote_core::clipboard::ClipboardSync>,
    /// The host's pointer, sent again when a window attaches.
    cursor: Option<Vec<u8>>,
    /// The direct connection's address once the session moved off the relay.
    direct: Option<String>,
    /// What the host allows, once it said so (older hosts never do).
    rights: Option<u32>,
    privacy: bool,
    /// Files last copied at the host, until fetched or replaced.
    host_files: Vec<String>,
    /// Set while the user records the session.
    recording: Option<Recording>,
    /// When the host's sound last arrived, to record it as well.
    last_audio: Option<std::time::Instant>,
}

/// A recording in progress: one file per screen size.
struct Recording {
    dir: std::path::PathBuf,
    base: String,
    current: Option<ctxremote_core::record::Recorder>,
    files: Vec<std::path::PathBuf>,
}

impl Recording {
    /// `sound`: the host's sound is on, so the file gets a sound track.
    fn frame(&mut self, frame: &VideoFrame, sound: bool) {
        // Another screen or size: this file ends, the next keyframe starts one.
        if self.current.as_ref().is_some_and(|r| !r.fits(frame)) {
            self.close();
        }
        match &mut self.current {
            Some(recorder) => {
                if let Err(e) = recorder.push(frame) {
                    tracing::warn!("Aufnahme unterbrochen: {e:#}");
                    self.current = None;
                }
            }
            None if frame.keyframe => {
                let name = match self.files.len() {
                    0 => format!("{}.mp4", self.base),
                    n => format!("{}-{}.mp4", self.base, n + 1),
                };
                match ctxremote_core::record::Recorder::start_with(&self.dir.join(name), frame, sound) {
                    Ok(recorder) => self.current = Some(recorder),
                    Err(e) => tracing::warn!("Aufnahme nicht startbar: {e:#}"),
                }
            }
            None => {}
        }
    }

    fn audio(&mut self, packet: &[u8]) {
        if let Some(recorder) = &mut self.current {
            if let Err(e) = recorder.push_audio(packet) {
                tracing::warn!("Ton der Aufnahme unterbrochen: {e:#}");
            }
        }
    }

    fn close(&mut self) {
        if let Some(recorder) = self.current.take() {
            match recorder.finish() {
                Ok(path) => self.files.push(path),
                Err(e) => tracing::warn!("Aufnahme nicht abgeschlossen: {e:#}"),
            }
        }
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Binary messages to session windows: a 12-byte header, then the payload.
const PACKET_VIDEO: u8 = 1;
const PACKET_CLOSED: u8 = 2;
const PACKET_CURSOR: u8 = 3;
/// One Opus packet of the host's sound (20 ms, 48 kHz stereo).
const PACKET_AUDIO: u8 = 4;

fn packet(kind: u8, keyframe: bool, width: u32, height: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + payload.len());
    out.extend_from_slice(&[kind, keyframe as u8, 0, 0]);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn video_packet(frame: &VideoFrame) -> Vec<u8> {
    packet(PACKET_VIDEO, frame.keyframe, frame.width, frame.height, &frame.data)
}

/// Header carries the size; the payload is the hotspot (2 × u32) and RGBA.
fn cursor_packet(shape: &CursorShape) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8 + shape.rgba.len());
    payload.extend_from_slice(&shape.hot_x.to_le_bytes());
    payload.extend_from_slice(&shape.hot_y.to_le_bytes());
    payload.extend_from_slice(&shape.rgba);
    packet(PACKET_CURSOR, false, shape.width, shape.height, &payload)
}

fn closed_packet(reason: &Option<String>) -> Vec<u8> {
    packet(PACKET_CLOSED, false, 0, 0, reason.as_deref().unwrap_or("").as_bytes())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    presence: Presence,
    password: String,
    server: String,
    unattended: bool,
    direct: DirectSettings,
    direct_active: bool,
    service: bool,
    host_supported: bool,
    peers: Vec<PeerView>,
    hosted: Vec<Hosted>,
    version: &'static str,
    /// `Permissions` bits for new sessions with the one-time password.
    rights_attended: u32,
    /// The same for the permanent password and the account's devices.
    rights_unattended: u32,
    /// The permanent password also needs the authenticator code.
    code_enabled: bool,
    /// Version of an available update the app can install, if any.
    update: Option<String>,
    /// This device's public alias, `None` if it has none.
    public_alias: Option<String>,
    /// Whether a public alias can be set (not in the portable helper).
    alias_supported: bool,
    /// How this user presents themselves when connecting to others.
    profile: Option<Profile>,
    /// Set while the address book syncs with an account.
    #[cfg(not(feature = "quick"))]
    account: Option<account::AccountView>,
    /// Devices of the account may connect to this one without a password.
    account_access: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerView {
    id: String,
    alias: Option<String>,
    name: String,
    last_seen: u64,
    /// It lets this account's devices in without a password.
    access: bool,
    /// Its network cards are known, so it can be woken.
    wake: bool,
    /// The user's groups for it.
    tags: Vec<String>,
    /// Online right now; `None` if not known (not a device of the account).
    online: Option<bool>,
}

#[derive(Serialize)]
struct Hosted {
    session: u64,
    peer: String,
    /// The viewer can receive chat messages.
    chat: bool,
    /// How the viewer presents itself, if it sent a profile (self-declared).
    profile: Option<Profile>,
    /// `Permissions` bits the viewer has.
    rights: u32,
    /// The screen here is blanked for the viewer (privacy mode).
    privacy: bool,
    /// The viewer records the session.
    recording: bool,
}

#[tauri::command]
fn overview(app: AppHandle, state: State<AppState>) -> Overview {
    #[cfg(feature = "quick")]
    let _ = &app;
    let config = state.config.read().unwrap();
    let (presence, password, server, unattended, sessions, service, direct, direct_active, chat, public_alias, profiles, account_access, rights, defaults, code_enabled, recording) = match &state.host {
        Side::Local(host) => (
            host.presence().borrow().clone(),
            host.password(),
            config.server.clone(),
            config.permanent_password.as_deref().is_some_and(|p| !p.is_empty()),
            host.sessions(),
            false,
            config.direct_settings(),
            host.direct_active(),
            host.chat_sessions(),
            host.public_alias(),
            host.session_profiles(),
            config.account_access.as_ref().is_some_and(|g| Some(g.host) == config.device_id),
            host.session_rights(),
            (config.rights_attended, config.rights_unattended),
            config.code_secret.is_some(),
            host.recording_sessions(),
        ),
        #[cfg(not(feature = "quick"))]
        Side::Service(service) => {
            let s = service.state();
            (s.presence, s.password, s.server, s.unattended, s.sessions, true, s.direct, s.direct_active, s.chat_sessions, s.public_alias, s.session_profiles, s.account_access, s.session_rights, (s.rights_attended, s.rights_unattended), s.code_enabled, s.session_recording)
        }
    };
    Overview {
        presence,
        password,
        server,
        unattended,
        direct,
        direct_active,
        service,
        host_supported: ctxremote_core::capture::HOST_SUPPORTED,
        peers: config
            .peers
            .iter()
            .map(|p| PeerView {
                id: p.id.to_string(),
                alias: p.alias.clone(),
                name: p.name.clone(),
                last_seen: p.last_seen,
                access: ctxremote_core::account::access_for(&config, p.id).is_some(),
                wake: !p.macs.is_empty(),
                tags: p.tags.clone(),
                #[cfg(not(feature = "quick"))]
                online: account::presence(&app, &p.id.to_string()),
                #[cfg(feature = "quick")]
                online: None,
            })
            .collect(),
        hosted: sessions
            .into_iter()
            .map(|(session, peer)| Hosted {
                session,
                peer,
                chat: chat.contains(&session),
                profile: profiles.iter().find(|(n, _)| *n == session).map(|(_, p)| p.clone()),
                rights: rights.iter().find(|(n, ..)| *n == session).map_or(0, |(_, r, _)| r.0),
                privacy: rights.iter().any(|(n, _, on)| *n == session && *on),
                recording: recording.contains(&session),
            })
            .collect(),
        version: ctxremote_core::update::VERSION,
        update: state.update.lock().unwrap().as_ref().map(|u| u.version.clone()),
        public_alias,
        alias_supported: !cfg!(feature = "quick"),
        profile: config.profile.clone(),
        #[cfg(not(feature = "quick"))]
        account: account::view(&app),
        account_access,
        rights_attended: defaults.0 .0,
        rights_unattended: defaults.1 .0,
        code_enabled,
    }
}

/// Who connected to this computer, newest first (see crates/core/src/history.rs).
#[tauri::command]
async fn history(state: State<'_, AppState>) -> CmdResult<Vec<ctxremote_core::history::Visit>> {
    match &state.host {
        Side::Local(host) => Ok(host.history()),
        #[cfg(not(feature = "quick"))]
        Side::Service(service) => service.history().await,
    }
}

#[tauri::command]
async fn refresh_password(state: State<'_, AppState>) -> CmdResult<String> {
    Ok(match &state.host {
        Side::Local(host) => {
            host.refresh_password();
            host.password()
        }
        #[cfg(not(feature = "quick"))]
        Side::Service(service) => service.refresh_password().await,
    })
}

/// Saves how this user presents themselves to others; an empty profile removes it.
/// Returns the profile as stored (cleaned up).
#[tauri::command]
fn save_profile(state: State<AppState>, profile: Profile) -> CmdResult<Option<Profile>> {
    let profile = profile.validate()?;
    let mut config = state.config.write().unwrap();
    config.profile = profile.clone();
    config.save().map_err(err)?;
    Ok(profile)
}

/// `permanent_password`: `None` keeps the current one, `Some("")` disables unattended access.
#[tauri::command]
async fn save_settings(
    state: State<'_, AppState>,
    server: String,
    permanent_password: Option<String>,
) -> CmdResult<()> {
    #[cfg_attr(feature = "quick", allow(clippy::infallible_destructuring_match))]
    let host = match &state.host {
        Side::Local(host) => host,
        // The service owns these settings; changing them needs an elevated helper.
        #[cfg(not(feature = "quick"))]
        Side::Service(_) => {
            service::configure(server.clone(), permanent_password).await?;
            // Viewing goes through the same server as hosting.
            let mut config = state.config.write().unwrap();
            config.server = server.trim().to_string();
            return config.save().map_err(err);
        }
    };
    let server_changed = {
        let mut config = state.config.write().unwrap();
        let changed = config.apply_settings(&server, permanent_password.as_deref()).map_err(err)?;
        config.save().map_err(err)?;
        changed
    };
    if server_changed {
        host.reconnect();
    }
    Ok(())
}

/// What new sessions may do. With the service this needs administrator
/// rights (UAC), as it shapes unattended access.
#[tauri::command]
async fn save_rights(state: State<'_, AppState>, attended: u32, unattended: u32) -> CmdResult<()> {
    let (attended, unattended) = (Permissions(attended), Permissions(unattended));
    #[cfg(not(feature = "quick"))]
    if let Side::Service(_) = &state.host {
        return service::configure_rights(attended, unattended).await;
    }
    let mut config = state.config.write().unwrap();
    config.rights_attended = attended;
    config.rights_unattended = unattended;
    config.save().map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CodeSetup {
    secret: String,
    /// The `otpauth://` link, also in the QR code.
    uri: String,
    /// The QR code as an SVG document.
    qr: String,
}

/// A new authenticator secret to show; nothing is stored until `code_enable`.
#[tauri::command]
fn code_setup() -> CmdResult<CodeSetup> {
    let secret = ctxremote_core::totp::new_secret();
    let uri = ctxremote_core::totp::uri(&secret, &ctxremote_core::account::default_label());
    let qr = qrcode::QrCode::new(uri.as_bytes())
        .map_err(err)?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(180, 180)
        .quiet_zone(true)
        .build();
    Ok(CodeSetup { secret, uri, qr })
}

/// Turns the code on once the app shows the right one, or off (`secret: None`).
/// With the service this needs administrator rights (UAC).
#[tauri::command]
async fn code_enable(state: State<'_, AppState>, secret: Option<String>, code: Option<String>) -> CmdResult<()> {
    if let Some(secret) = &secret {
        let code = code.unwrap_or_default();
        if ctxremote_core::totp::check(secret, &code, ctxremote_core::totp::now()).is_none() {
            return Err("Der Code passt nicht. Stimmt die Uhrzeit auf beiden Geräten?".into());
        }
    }
    #[cfg(not(feature = "quick"))]
    if let Side::Service(_) = &state.host {
        return service::configure_code(secret).await;
    }
    let mut config = state.config.write().unwrap();
    config.code_secret = secret;
    config.save().map_err(err)
}

/// Direct-connection settings; not available in the quick build, which never listens.
#[tauri::command]
async fn save_direct(state: State<'_, AppState>, settings: DirectSettings) -> CmdResult<()> {
    #[cfg(feature = "quick")]
    {
        let _ = (state, settings);
        Err("Die Schnellhilfe nimmt keine Direktverbindungen an".into())
    }
    #[cfg(not(feature = "quick"))]
    {
        let host = match &state.host {
            Side::Local(host) => host,
            Side::Service(_) => {
                // The service validates and applies; the new state arrives over the link.
                return service::configure_direct(settings).await;
            }
        };
        let changed = {
            let mut config = state.config.write().unwrap();
            let changed = config.apply_direct(&settings).map_err(err)?;
            config.save().map_err(err)?;
            changed
        };
        if changed {
            host.set_direct(settings);
            // Lets the listener settle, so the overview shows the real state.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
        Ok(())
    }
}

/// Sets (or with `None` drops) this device's public alias; returns it as stored.
#[tauri::command]
async fn set_public_alias(state: State<'_, AppState>, alias: Option<String>) -> CmdResult<Option<String>> {
    match &state.host {
        // The portable helper gets a new identity every run; an alias would dangle.
        #[cfg(feature = "quick")]
        Side::Local(_) => {
            let _ = alias;
            Err("In der Schnellhilfe gibt es keinen Alias".into())
        }
        #[cfg(not(feature = "quick"))]
        Side::Local(host) => host.set_public_alias(alias).await,
        #[cfg(not(feature = "quick"))]
        Side::Service(service) => service.set_public_alias(alias).await,
    }
}

/// Sends the Wake-on-LAN packet for a known device into the local networks
/// and asks the account's online devices to do the same in theirs. Returns
/// how many of those were asked.
#[tauri::command]
async fn wake_peer(state: State<'_, AppState>, id: String) -> CmdResult<u32> {
    let id: DeviceId = id.parse().map_err(err)?;
    let (macs, account, server, key) = {
        let config = state.config.read().unwrap();
        let macs = config.peer(id).map(|p| p.macs.clone()).unwrap_or_default();
        (macs, config.account.is_some(), config.server_addr(), config.signing_key().map_err(err)?)
    };
    let local = {
        let macs = macs.clone();
        tauri::async_runtime::spawn_blocking(move || ctxremote_core::wol::wake(&macs)).await.map_err(err)?
    };
    let asked = if account { ctxremote_core::account::wake(&server, &key, macs).await.unwrap_or(0) } else { 0 };
    // Only an error if the packet went nowhere at all.
    match (local, asked) {
        (Err(e), 0) => Err(chain(e)),
        _ => Ok(asked),
    }
}

/// Puts a known device into groups (shared through the account).
#[tauri::command]
fn set_tags(app: AppHandle, state: State<AppState>, id: String, tags: Vec<String>) -> CmdResult<()> {
    let id: DeviceId = id.parse().map_err(err)?;
    let mut config = state.config.write().unwrap();
    config.set_tags(id, &tags).map_err(err)?;
    config.save().map_err(err)?;
    poke_sync(&app);
    Ok(())
}

#[tauri::command]
fn forget_peer(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<()> {
    let id: DeviceId = id.parse().map_err(err)?;
    let mut config = state.config.write().unwrap();
    config.forget(id);
    config.save().map_err(err)?;
    poke_sync(&app);
    Ok(())
}

#[tauri::command]
fn set_alias(app: AppHandle, state: State<AppState>, id: String, alias: Option<String>) -> CmdResult<()> {
    let id: DeviceId = id.parse().map_err(err)?;
    let mut config = state.config.write().unwrap();
    config.set_alias(id, alias.as_deref()).map_err(err)?;
    config.save().map_err(err)?;
    poke_sync(&app);
    Ok(())
}

/// Lets the address book reach the account soon after a change.
fn poke_sync(app: &AppHandle) {
    #[cfg(not(feature = "quick"))]
    app.state::<account::Sync>().poke();
    let _ = app;
}

#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    target: String,
    password: String,
    code: Option<String>,
) -> CmdResult<u32> {
    let (server, own_id, local) = {
        let config = state.config.read().unwrap();
        (config.server_addr(), config.device_id, config.resolve(&target))
    };
    // An ID or one of the user's own names; otherwise a public alias on the server.
    let target = match local {
        Some(id) => id,
        None => {
            if ctxremote_core::alias::normalize_alias(&target).is_err() {
                return Err("Ungültige ID oder ungültiger Alias".into());
            }
            ctxremote_core::alias::resolve(&server, &target)
                .await
                .map_err(chain)?
                .ok_or_else(|| format!("Den Alias „{}“ gibt es nicht", target.trim()))?
        }
    };
    if own_id == Some(target) {
        return Err("Das ist die ID dieses Geräts".into());
    }

    let number = state.next_viewer.fetch_add(1, Ordering::Relaxed);
    let link = Arc::new(Mutex::new(Link::default()));
    let on_event = {
        let link = link.clone();
        let app = app.clone();
        move |event: ViewerEvent| match event {
            ViewerEvent::Video(frame) => {
                let mut link = link.lock().unwrap();
                if let Some(channel) = &link.channel {
                    let _ = channel.send(InvokeResponseBody::Raw(video_packet(&frame)));
                }
                let sound = link.last_audio.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(2));
                if let Some(recording) = &mut link.recording {
                    recording.frame(&frame, sound);
                }
            }
            ViewerEvent::Cursor(shape) => {
                let packet = cursor_packet(&shape);
                let mut link = link.lock().unwrap();
                if let Some(channel) = &link.channel {
                    let _ = channel.send(InvokeResponseBody::Raw(packet.clone()));
                }
                link.cursor = Some(packet);
            }
            ViewerEvent::Audio(data) => {
                let mut link = link.lock().unwrap();
                if let Some(channel) = &link.channel {
                    let _ = channel.send(InvokeResponseBody::Raw(packet(PACKET_AUDIO, false, 0, 0, &data)));
                }
                link.last_audio = Some(std::time::Instant::now());
                if let Some(recording) = &mut link.recording {
                    recording.audio(&data);
                }
            }
            ViewerEvent::Chat(text) => {
                let _ = app.emit_to(format!("session-{number}"), "chat", text);
            }
            // Kept for a window that attaches later; it comes right after `Welcome`.
            ViewerEvent::Rights(rights) => {
                link.lock().unwrap().rights = Some(rights.0);
                let _ = app.emit_to(format!("session-{number}"), "rights", rights.0);
            }
            ViewerEvent::SystemInfo(info) => {
                // Its network cards, to wake it later (and on the account's other devices).
                let macs: Vec<String> = info.networks.iter().map(|n| n.mac.clone()).collect();
                let changed = {
                    let state = app.state::<AppState>();
                    let mut config = state.config.write().unwrap();
                    let changed = config.set_macs(target, macs);
                    if changed {
                        let _ = config.save();
                    }
                    changed
                };
                if changed {
                    poke_sync(&app);
                }
                let _ = app.emit_to(format!("session-{number}"), "system-info", info);
            }
            ViewerEvent::ClipboardFiles(paths) => {
                let names: Vec<String> = paths
                    .iter()
                    .map(|p| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string())
                    .collect();
                link.lock().unwrap().host_files = paths;
                let _ = app.emit_to(format!("session-{number}"), "host-files", names);
            }
            ViewerEvent::Privacy { on, error } => {
                link.lock().unwrap().privacy = on;
                let _ = app.emit_to(format!("session-{number}"), "privacy", PrivacyUpdate { on, error });
            }
            ViewerEvent::Direct(addr) => {
                link.lock().unwrap().direct = Some(addr.clone());
                let _ = app.emit_to(format!("session-{number}"), "route", addr);
            }
            ViewerEvent::Transfer(event) => {
                let _ = app.emit("transfer", TransferUpdate { session: number, event });
            }
            ViewerEvent::Clipboard(text) => {
                if let Some(clipboard) = &link.lock().unwrap().clipboard {
                    clipboard.apply(text);
                }
            }
            ViewerEvent::Closed(reason) => {
                {
                    let mut link = link.lock().unwrap();
                    link.clipboard = None;
                    if let Some(recording) = &mut link.recording {
                        recording.close();
                    }
                    if let Some(channel) = &link.channel {
                        let _ = channel.send(InvokeResponseBody::Raw(closed_packet(&reason)));
                    }
                    link.closed = Some(reason);
                }
                // Lock order is viewers → link, so the link lock is released first.
                app.state::<AppState>().viewers.lock().unwrap().remove(&number);
            }
        }
    };

    // No password: a device that lets this account's devices in (docs/ACCOUNTS.md).
    let (password, member, profile) = {
        let config = state.config.read().unwrap();
        let profile = config.profile.as_ref().and_then(Profile::to_wire);
        if password.is_empty() {
            let access = ctxremote_core::account::access_for(&config, target).ok_or("Bitte das Passwort eingeben")?;
            (access, Some(config.signing_key().map_err(err)?), profile)
        } else {
            (password, None, profile)
        }
    };
    let code = code.filter(|c| !c.trim().is_empty());
    let session = ViewerSession::connect(&server, own_id, target, &password, member.as_ref(), profile, code.as_deref(), on_event)
        .await
        .map_err(|e| format!("{e:#}"))?;
    // Once per session: the info panel opens filled, and the MACs are learned.
    session.request_system_info();
    link.lock().unwrap().clipboard = {
        let outbox = session.sender();
        ctxremote_core::clipboard::ClipboardSync::start(true, move |text| {
            let _ = outbox.send(ViewerMsg::Clipboard(text));
        })
    };
    let label = {
        let mut config = state.config.write().unwrap();
        config.remember(target, &session.host.hostname);
        let _ = config.save();
        config.peer(target).map_or(session.host.hostname.clone(), |p| p.label().to_string())
    };
    poke_sync(&app);
    state.viewers.lock().unwrap().insert(number, Viewer { session, target, link });

    WebviewWindowBuilder::new(
        &app,
        format!("session-{number}"),
        WebviewUrl::App(format!("index.html#/session/{number}").into()),
    )
    .title(format!("{label} · {target}"))
    .inner_size(1280.0, 800.0)
    .min_inner_size(640.0, 400.0)
    .build()
    .map_err(err)?;
    Ok(number)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Attached {
    host: HostInfo,
    id: String,
    label: String,
    /// What the host supports; buttons for anything else stay hidden.
    features: Features,
    /// Address of the direct connection, `None` while on the relay.
    direct: Option<String>,
    /// `Permissions` bits the host allows; `None` from hosts that do not say.
    rights: Option<u32>,
    privacy: bool,
}

#[derive(Clone, Serialize)]
struct PrivacyUpdate {
    on: bool,
    error: Option<String>,
}

/// The host's capabilities by name, for the session window.
#[derive(Serialize)]
struct Features {
    files: bool,
    restart: bool,
    quality: bool,
    chat: bool,
    audio: bool,
    privacy: bool,
    file_paste: bool,
    sysinfo: bool,
    recording: bool,
    tunnel: bool,
    draw: bool,
    mic: bool,
}

impl From<ctxremote_core::proto::session::Features> for Features {
    fn from(f: ctxremote_core::proto::session::Features) -> Self {
        use ctxremote_core::proto::session::Features as F;
        Self {
            files: f.has(F::FILES),
            restart: f.has(F::RESTART),
            quality: f.has(F::QUALITY),
            chat: f.has(F::CHAT),
            audio: f.has(F::AUDIO),
            privacy: f.has(F::PRIVACY),
            file_paste: f.has(F::FILE_PASTE),
            sysinfo: f.has(F::SYSINFO),
            recording: f.has(F::RECORDING),
            tunnel: f.has(F::TUNNEL),
            draw: f.has(F::DRAW),
            mic: f.has(F::MIC),
        }
    }
}

#[tauri::command]
fn attach(
    state: State<AppState>,
    session: u32,
    channel: Channel<InvokeResponseBody>,
) -> CmdResult<Attached> {
    let viewers = state.viewers.lock().unwrap();
    let viewer = viewers.get(&session).ok_or("Die Sitzung ist bereits beendet")?;
    let mut link = viewer.link.lock().unwrap();
    if let Some(reason) = &link.closed {
        let _ = channel.send(InvokeResponseBody::Raw(closed_packet(reason)));
    }
    if let Some(cursor) = &link.cursor {
        let _ = channel.send(InvokeResponseBody::Raw(cursor.clone()));
    }
    link.channel = Some(channel);
    viewer.session.send(ViewerMsg::RequestKeyframe);
    let label = state
        .config
        .read()
        .unwrap()
        .peer(viewer.target)
        .map_or(viewer.session.host.hostname.clone(), |p| p.label().to_string());
    let direct = link.direct.clone();
    Ok(Attached {
        host: viewer.session.host.clone(),
        id: viewer.target.to_string(),
        label,
        features: viewer.session.features.into(),
        direct,
        rights: link.rights,
        privacy: link.privacy,
    })
}

fn with_viewer(state: &AppState, session: u32, f: impl FnOnce(&ViewerSession)) {
    if let Some(viewer) = state.viewers.lock().unwrap().get(&session) {
        f(&viewer.session);
    }
}

#[tauri::command]
fn send_input(state: State<AppState>, session: u32, event: InputEvent) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::Input(event)));
}

#[tauri::command]
fn select_display(state: State<AppState>, session: u32, index: u8) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::SelectDisplay(index)));
}

#[tauri::command]
fn request_keyframe(state: State<AppState>, session: u32) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::RequestKeyframe));
}

/// Turns the host's sound on or off for `session`; false if the host has none.
#[tauri::command]
fn set_audio(state: State<AppState>, session: u32, on: bool) -> bool {
    let mut supported = false;
    with_viewer(&state, session, |s| supported = s.set_audio(on));
    supported
}

/// Starts recording the session into the video folder; returns the folder.
/// The host is told, so the person there sees it.
#[tauri::command]
fn start_recording(state: State<AppState>, session: u32) -> CmdResult<String> {
    let viewers = state.viewers.lock().unwrap();
    let viewer = viewers.get(&session).ok_or("Die Sitzung ist beendet")?;
    let dir = ctxremote_core::record::default_dir();
    let name = state.config.read().unwrap().peer(viewer.target).map_or(viewer.session.host.hostname.clone(), |p| p.label().to_string());
    let base = format!(
        "{}_{}",
        ctxremote_core::record::safe_name(&name),
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
    );
    viewer.link.lock().unwrap().recording = Some(Recording { dir: dir.clone(), base, current: None, files: Vec::new() });
    // A file starts with a keyframe.
    viewer.session.send(ViewerMsg::RequestKeyframe);
    viewer.session.set_recording(true);
    Ok(dir.to_string_lossy().into_owned())
}

/// Ends the recording; returns the files written.
#[tauri::command]
fn stop_recording(state: State<AppState>, session: u32) -> CmdResult<Vec<String>> {
    let viewers = state.viewers.lock().unwrap();
    let viewer = viewers.get(&session).ok_or("Die Sitzung ist beendet")?;
    let recording = viewer.link.lock().unwrap().recording.take();
    viewer.session.set_recording(false);
    let Some(mut recording) = recording else { return Ok(Vec::new()) };
    recording.close();
    Ok(recording.files.iter().map(|p| p.to_string_lossy().into_owned()).collect())
}

/// Opens a port tunnel: `127.0.0.1:port` here (0: any free port) leads to
/// `target` in the host's network. Returns the port.
#[tauri::command]
async fn open_tunnel(state: State<'_, AppState>, session: u32, port: Option<u16>, target: String) -> CmdResult<u16> {
    let tunnels = {
        let viewers = state.viewers.lock().unwrap();
        let viewer = viewers.get(&session).ok_or("Die Sitzung ist beendet")?;
        viewer.session.tunnels().clone()
    };
    tunnels.open(port.unwrap_or(0), &target).await.map_err(chain)
}

#[tauri::command]
fn close_tunnel(state: State<AppState>, session: u32, port: u16) {
    with_viewer(&state, session, |s| s.tunnels().close(port));
}

#[tauri::command]
fn list_tunnels(state: State<AppState>, session: u32) -> Vec<ctxremote_core::tunnel::TunnelInfo> {
    let mut list = Vec::new();
    with_viewer(&state, session, |s| list = s.tunnels().list());
    list
}

/// One Opus packet from this computer's microphone for the host's speaker,
/// as raw bytes; the session number comes in the `session` header.
#[tauri::command]
fn mic_packet(state: State<AppState>, request: tauri::ipc::Request<'_>) -> CmdResult<()> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("Erwartet Rohdaten".into());
    };
    let session: u32 = request
        .headers()
        .get("session")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or("Sitzung fehlt")?;
    with_viewer(&state, session, |s| {
        if s.features.has(ctxremote_core::proto::session::Features::MIC) {
            s.send(ViewerMsg::Mic(ctxremote_core::proto::session::AudioPacket { data: data.clone() }));
        }
    });
    Ok(())
}

/// A line drawn over the host's screen (points 0..=65535), or `None` to
/// clear all lines.
#[tauri::command]
fn draw(state: State<AppState>, session: u32, color: Option<u32>, width: Option<u8>, points: Option<Vec<(u16, u16)>>) {
    use ctxremote_core::proto::session::DrawMsg;
    let msg = match (color, width, points) {
        (Some(color), Some(width), Some(points)) => DrawMsg::Stroke { color, width, points },
        _ => DrawMsg::Clear,
    };
    with_viewer(&state, session, |s| {
        if s.features.has(ctxremote_core::proto::session::Features::DRAW) {
            s.send(ViewerMsg::Draw(msg));
        }
    });
}

/// Asks the host about its computer; the answer arrives as `system-info`.
#[tauri::command]
fn request_system_info(state: State<AppState>, session: u32) -> bool {
    let mut supported = false;
    with_viewer(&state, session, |s| supported = s.request_system_info());
    supported
}

/// Turns privacy mode on the host on or off; false if the host has none.
#[tauri::command]
fn set_privacy(state: State<AppState>, session: u32, on: bool) -> bool {
    let mut supported = false;
    with_viewer(&state, session, |s| supported = s.set_privacy(on));
    supported
}

#[tauri::command]
fn lock_screen(state: State<AppState>, session: u32) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::LockScreen));
}

/// Chat from this viewer to the host of `session`.
#[tauri::command]
fn send_chat(state: State<AppState>, session: u32, text: String) -> CmdResult<()> {
    let text = ctxremote_core::host::chat_text(&text).ok_or("Leere Nachricht")?;
    with_viewer(&state, session, |s| s.send(ViewerMsg::Chat(text)));
    Ok(())
}

/// Chat from the person at this device to the viewer of a hosted session.
#[tauri::command]
fn host_chat(state: State<AppState>, session: u64, text: String) -> CmdResult<()> {
    state.host.send_chat(session, text)
}

#[tauri::command]
fn set_quality(state: State<AppState>, session: u32, quality: Quality) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::SetQuality(quality)));
}

#[tauri::command]
fn restart_host(state: State<AppState>, session: u32) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::Restart));
}

#[tauri::command]
fn send_sas(state: State<AppState>, session: u32) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::SecureAttention));
}

/// A transfer event for the file window of `session`.
#[derive(Serialize, Clone)]
struct TransferUpdate {
    session: u32,
    #[serde(flatten)]
    event: TransferEvent,
}

/// Opens (or focuses) the file transfer window of a session.
///
/// Async on purpose: building a window inside a synchronous command blocks the
/// main thread on Windows, so the new window stays white and the app freezes.
#[tauri::command]
async fn open_files(app: AppHandle, state: State<'_, AppState>, session: u32) -> CmdResult<()> {
    show_files_window(&app, &state, session)
}

fn show_files_window(app: &AppHandle, state: &AppState, session: u32) -> CmdResult<()> {
    let label = format!("files-{session}");
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        return window.set_focus().map_err(err);
    }
    let title = {
        let viewers = state.viewers.lock().unwrap();
        let viewer = viewers.get(&session).ok_or("Die Sitzung ist bereits beendet")?;
        let name = state
            .config
            .read()
            .unwrap()
            .peer(viewer.target)
            .map_or(viewer.session.host.hostname.clone(), |p| p.label().to_string());
        format!("Dateien · {name}")
    };
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("index.html#/files/{session}").into()))
        .title(title)
        .inner_size(1040.0, 640.0)
        .min_inner_size(760.0, 420.0)
        .build()
        .map_err(err)?;
    Ok(())
}

/// Files dropped on the session window: the file window uploads them, so their
/// progress shows there. It picks them up with `take_drops` when it opens or is told.
/// Async for the same reason as `open_files`.
#[tauri::command]
async fn queue_drop(app: AppHandle, state: State<'_, AppState>, session: u32, paths: Vec<String>) -> CmdResult<()> {
    state.drops.lock().unwrap().entry(session).or_default().extend(paths);
    show_files_window(&app, &state, session)?;
    let _ = app.emit_to(format!("files-{session}"), "files-drop", session);
    Ok(())
}

#[tauri::command]
fn take_drops(state: State<AppState>, session: u32) -> Vec<String> {
    state.drops.lock().unwrap().remove(&session).unwrap_or_default()
}

/// Ctrl+V in a session window: if files are on this computer's clipboard,
/// they go to the host and onto its clipboard first. Returns whether there
/// were files; the window then sends Ctrl+V itself.
#[tauri::command]
async fn paste_files(state: State<'_, AppState>, session: u32) -> CmdResult<bool> {
    let local = tauri::async_runtime::spawn_blocking(ctxremote_core::clipboard::local_files).await.map_err(err)?;
    if local.is_empty() {
        return Ok(false);
    }
    let client = file_client(&state, session)?;
    client.paste_to_host(local).await.map_err(chain)?;
    Ok(true)
}

/// Fetches the files last copied at the host and puts them on this
/// computer's clipboard. Returns how many arrived.
#[tauri::command]
async fn fetch_host_files(state: State<'_, AppState>, session: u32) -> CmdResult<usize> {
    let paths = {
        let viewers = state.viewers.lock().unwrap();
        let viewer = viewers.get(&session).ok_or("Die Sitzung ist beendet")?;
        let paths = viewer.link.lock().unwrap().host_files.clone();
        paths
    };
    if paths.is_empty() {
        return Err("Am Gerät sind keine Dateien kopiert".into());
    }
    let client = file_client(&state, session)?;
    let landed = client.fetch_to_clipboard(paths).await.map_err(chain)?;
    let count = landed.len();
    tauri::async_runtime::spawn_blocking(move || ctxremote_core::clipboard::set_local_files(&landed))
        .await
        .map_err(err)?
        .map_err(chain)?;
    Ok(count)
}

fn file_client(state: &AppState, session: u32) -> CmdResult<Arc<FileClient>> {
    let viewers = state.viewers.lock().unwrap();
    let viewer = viewers.get(&session).ok_or("Die Sitzung ist beendet")?;
    Ok(viewer.session.files().clone())
}

fn chain(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
async fn remote_list(state: State<'_, AppState>, session: u32, path: String) -> CmdResult<Listing> {
    file_client(&state, session)?.list(path).await.map_err(chain)
}

#[tauri::command]
async fn remote_create_dir(state: State<'_, AppState>, session: u32, path: String) -> CmdResult<()> {
    file_client(&state, session)?.create_dir(path).await.map_err(chain)
}

#[tauri::command]
async fn remote_rename(state: State<'_, AppState>, session: u32, path: String, name: String) -> CmdResult<()> {
    file_client(&state, session)?.rename(path, name).await.map_err(chain)
}

#[tauri::command]
async fn remote_delete(state: State<'_, AppState>, session: u32, paths: Vec<String>) -> CmdResult<()> {
    file_client(&state, session)?.delete(paths).await.map_err(chain)
}

/// Sends a local file or folder into the host's folder `dir`; returns the transfer id.
#[tauri::command]
async fn upload(state: State<'_, AppState>, session: u32, path: String, dir: String) -> CmdResult<u32> {
    file_client(&state, session)?.upload(path.into(), dir).await.map_err(chain)
}

/// Fetches the host's file or folder `path` into the local folder `dir`.
#[tauri::command]
async fn download(state: State<'_, AppState>, session: u32, path: String, dir: String) -> CmdResult<u32> {
    file_client(&state, session)?.download(path, dir.into()).await.map_err(chain)
}

#[tauri::command]
fn cancel_transfer(state: State<AppState>, session: u32, id: u32) {
    if let Ok(files) = file_client(&state, session) {
        files.cancel(id);
    }
}

/// Runs local file work off the UI thread.
async fn local<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?.map_err(chain)
}

#[tauri::command]
async fn local_list(path: String) -> CmdResult<Listing> {
    local(move || files::list(&path, &UserContext::current())).await
}

#[tauri::command]
async fn local_create_dir(path: String) -> CmdResult<()> {
    local(move || files::create_dir(&path)).await
}

#[tauri::command]
async fn local_rename(path: String, name: String) -> CmdResult<()> {
    local(move || files::rename(&path, &name)).await
}

#[tauri::command]
async fn local_delete(paths: Vec<String>) -> CmdResult<()> {
    local(move || files::delete(&paths)).await
}

/// Where the local pane starts: the user's downloads, or the top level.
#[tauri::command]
fn local_home() -> String {
    files::client::default_download_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Shows a local file or folder in the system's file manager.
#[tauri::command]
fn reveal(path: String) -> CmdResult<()> {
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg("-R").arg(&path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = {
        let dir = std::path::Path::new(&path).parent().map(|p| p.to_path_buf()).unwrap_or_default();
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };
    result.map(|_| ()).map_err(err)
}

#[tauri::command]
fn disconnect(state: State<AppState>, session: u32) {
    // Dropping the session sends `Bye`.
    state.viewers.lock().unwrap().remove(&session);
}

#[tauri::command]
fn end_hosted_session(state: State<AppState>, session: u64) {
    state.host.end_session(session);
}

/// Changes what the viewer of a session at this computer may do.
#[tauri::command]
fn set_hosted_rights(state: State<AppState>, session: u64, rights: u32) -> CmdResult<()> {
    state.host.set_rights(session, Permissions(rights))
}

/// Downloads the available update and starts its installer, which asks for
/// administrator rights, closes this app and starts the new version.
#[tauri::command]
async fn install_update(state: State<'_, AppState>) -> CmdResult<()> {
    let info = state.update.lock().unwrap().clone().ok_or("Kein Update verfügbar")?;
    let server = state.config.read().unwrap().server_addr();
    let dir = std::env::temp_dir().join("ctxremote-update");
    let _ = std::fs::remove_dir_all(&dir);
    let installer = ctxremote_core::update::download(&server, &info, &dir).await.map_err(chain)?;
    ctxremote_core::update::launch(&installer, ctxremote_core::update::InstallMode::Interactive).map_err(chain)
}

/// App mode without the service: looks for updates now and then and tells the window.
#[cfg(not(feature = "quick"))]
fn watch_updates(app: &AppHandle) {
    if !ctxremote_core::update::enabled() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            let server = app.state::<AppState>().config.read().unwrap().server_addr();
            match ctxremote_core::update::check(&server).await {
                Ok(Some(info)) => {
                    let version = info.version.clone();
                    *app.state::<AppState>().update.lock().unwrap() = Some(info);
                    let _ = app.emit("update-available", version);
                }
                Ok(None) => {}
                Err(e) => tracing::info!("Update-Prüfung fehlgeschlagen: {e:#}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Registers the commands; `$extra` adds build-specific ones.
macro_rules! handlers {
    ($($extra:path),*) => {
        tauri::generate_handler![
            overview,
            refresh_password,
            history,
            save_settings,
            save_rights,
            code_setup,
            code_enable,
            save_direct,
            forget_peer,
            set_tags,
            wake_peer,
            set_alias,
            connect,
            attach,
            send_input,
            select_display,
            request_keyframe,
            set_audio,
            set_privacy,
            request_system_info,
            draw,
            mic_packet,
            open_tunnel,
            close_tunnel,
            list_tunnels,
            start_recording,
            stop_recording,
            paste_files,
            fetch_host_files,
            send_sas,
            lock_screen,
            restart_host,
            send_chat,
            host_chat,
            set_quality,
            disconnect,
            end_hosted_session,
            set_hosted_rights,
            save_profile,
            install_update,
            set_public_alias,
            open_files,
            queue_drop,
            take_drops,
            remote_list,
            remote_create_dir,
            remote_rename,
            remote_delete,
            upload,
            download,
            cancel_transfer,
            local_list,
            local_create_dir,
            local_rename,
            local_delete,
            local_home,
            reveal,
            $($extra),*
        ]
    };
}

fn on_run_event(_app: &AppHandle, event: RunEvent) {
    #[cfg(feature = "quick")]
    if let RunEvent::Exit = event {
        quick::cleanup();
    }
    #[cfg(not(feature = "quick"))]
    let _ = event;
}

#[cfg(not(feature = "quick"))]
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "CTXRemote öffnen", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("bundle has an icon"))
        .tooltip("CTXRemote")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Passes the local host's presence and events on to the windows.
fn forward_local_events(app: &AppHandle, host: &Host) {
    let handle = app.clone();
    let mut presence = host.presence();
    tauri::async_runtime::spawn(async move {
        loop {
            let current = presence.borrow_and_update().clone();
            let _ = handle.emit("presence", current);
            if presence.changed().await.is_err() {
                break;
            }
        }
    });

    let handle = app.clone();
    let mut events = host.events();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(event) => {
                    if matches!(event, ctxremote_core::host::HostEvent::SessionStarted { .. } | ctxremote_core::host::HostEvent::Chat { .. }) {
                        show_main(&handle);
                    }
                    let _ = handle.emit("host-event", event);
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ctxremote=debug".into()),
        )
        .init();

    #[cfg(feature = "quick")]
    quick::pin_config();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // A separate config is a separate profile (e.g. a second device for testing on one PC).
    #[cfg(not(feature = "quick"))]
    if std::env::var_os("CTXREMOTE_CONFIG").is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| show_main(app)));
    }
    #[cfg(feature = "quick")]
    let commands: fn(tauri::ipc::Invoke) -> bool = handlers![quick::answer_approval];
    #[cfg(not(feature = "quick"))]
    let commands: fn(tauri::ipc::Invoke) -> bool = handlers![
        account::account_create,
        account::account_pairing_code,
        account::account_join,
        account::account_leave,
        account::account_sync,
        account::account_register,
        account::account_login,
        account::account_recover,
        account::account_set_login,
        account::account_details,
        account::account_remove_device,
        account::account_set_access
    ];
    #[cfg(feature = "quick")]
    let builder = builder.manage(quick::Approvals::default());
    #[cfg(not(feature = "quick"))]
    let builder = builder.manage(account::Sync::default());

    builder
        .setup(|app| {
            #[cfg(feature = "quick")]
            let config = Arc::new(RwLock::new(quick::load_config()?));
            #[cfg(not(feature = "quick"))]
            let config = Arc::new(RwLock::new(Config::load()?));
            let host = tauri::async_runtime::block_on(async {
                // Service mode, unless this is the portable build or a separate profile.
                #[cfg(not(feature = "quick"))]
                if std::env::var_os("CTXREMOTE_CONFIG").is_none() {
                    if let Some(service) = service::Service::detect(app.handle()).await {
                        return Side::Service(service);
                    }
                }
                Side::Local(Host::start(config.clone()))
            });
            #[cfg_attr(feature = "quick", allow(irrefutable_let_patterns))]
            if let Side::Local(host) = &host {
                #[cfg(feature = "quick")]
                host.require_approval(quick::approver(app.handle().clone()));
                forward_local_events(app.handle(), host);
            }

            app.manage(AppState {
                config,
                host,
                viewers: Mutex::default(),
                next_viewer: AtomicU32::new(1),
                drops: Mutex::default(),
                update: Mutex::default(),
            });

            #[cfg(not(feature = "quick"))]
            build_tray(app)?;
            #[cfg(not(feature = "quick"))]
            account::start(app.handle());
            // With the service installed, the service updates everything itself.
            #[cfg(not(feature = "quick"))]
            if matches!(app.state::<AppState>().host, Side::Local(_)) {
                watch_updates(app.handle());
            }

            // `--tray`: started by the service after an update; stay in the tray.
            if !std::env::args().any(|a| a == "--tray") {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    // Quick: closing the window ends the app and any running session.
                    #[cfg(feature = "quick")]
                    {
                        let _ = api;
                        window.app_handle().exit(0);
                    }
                    // Closing the main window keeps the device reachable from the tray.
                    #[cfg(not(feature = "quick"))]
                    {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
            if let WindowEvent::Destroyed = event {
                if let Some(number) = window.label().strip_prefix("session-").and_then(|n| n.parse().ok()) {
                    window.state::<AppState>().viewers.lock().unwrap().remove(&number);
                }
            }
        })
        .invoke_handler(commands)
        .build(tauri::generate_context!())
        .expect("CTXRemote konnte nicht gestartet werden")
        .run(on_run_event);
}
