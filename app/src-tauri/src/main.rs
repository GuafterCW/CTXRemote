#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "quick")]
mod quick;
#[cfg(not(feature = "quick"))]
mod service;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use ctxremote_core::config::Config;
use ctxremote_core::files::client::{FileClient, TransferEvent};
use ctxremote_core::files::{self, UserContext};
use ctxremote_core::host::{Host, Presence};
use ctxremote_core::proto::session::{CursorShape, HostInfo, InputEvent, Listing, Quality, VideoFrame, ViewerMsg};
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
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Binary messages to session windows: a 12-byte header, then the payload.
const PACKET_VIDEO: u8 = 1;
const PACKET_CLOSED: u8 = 2;
const PACKET_CURSOR: u8 = 3;

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
    service: bool,
    host_supported: bool,
    peers: Vec<PeerView>,
    hosted: Vec<Hosted>,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerView {
    id: String,
    alias: Option<String>,
    name: String,
    last_seen: u64,
}

#[derive(Serialize)]
struct Hosted {
    session: u64,
    peer: String,
}

#[tauri::command]
fn overview(state: State<AppState>) -> Overview {
    let config = state.config.read().unwrap();
    let (presence, password, server, unattended, sessions, service) = match &state.host {
        Side::Local(host) => (
            host.presence().borrow().clone(),
            host.password(),
            config.server.clone(),
            config.permanent_password.as_deref().is_some_and(|p| !p.is_empty()),
            host.sessions(),
            false,
        ),
        #[cfg(not(feature = "quick"))]
        Side::Service(service) => {
            let s = service.state();
            (s.presence, s.password, s.server, s.unattended, s.sessions, true)
        }
    };
    Overview {
        presence,
        password,
        server,
        unattended,
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
            })
            .collect(),
        hosted: sessions.into_iter().map(|(session, peer)| Hosted { session, peer }).collect(),
        version: env!("CARGO_PKG_VERSION"),
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

#[tauri::command]
fn forget_peer(state: State<AppState>, id: String) -> CmdResult<()> {
    let id: DeviceId = id.parse().map_err(err)?;
    let mut config = state.config.write().unwrap();
    config.forget(id);
    config.save().map_err(err)
}

#[tauri::command]
fn set_alias(state: State<AppState>, id: String, alias: Option<String>) -> CmdResult<()> {
    let id: DeviceId = id.parse().map_err(err)?;
    let mut config = state.config.write().unwrap();
    config.set_alias(id, alias.as_deref()).map_err(err)?;
    config.save().map_err(err)
}

#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    target: String,
    password: String,
) -> CmdResult<u32> {
    let (server, own_id, target) = {
        let config = state.config.read().unwrap();
        let target = config
            .resolve(&target)
            .ok_or("Unbekannter Alias oder ungültige ID")?;
        (config.server_addr(), config.device_id, target)
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
                if let Some(channel) = &link.lock().unwrap().channel {
                    let _ = channel.send(InvokeResponseBody::Raw(video_packet(&frame)));
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

    let session = ViewerSession::connect(&server, own_id, target, &password, on_event)
        .await
        .map_err(|e| format!("{e:#}"))?;
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
    Ok(Attached { host: viewer.session.host.clone(), id: viewer.target.to_string(), label })
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

#[tauri::command]
fn lock_screen(state: State<AppState>, session: u32) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::LockScreen));
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
#[tauri::command]
fn open_files(app: AppHandle, state: State<AppState>, session: u32) -> CmdResult<()> {
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
    WebviewWindowBuilder::new(&app, label, WebviewUrl::App(format!("index.html#/files/{session}").into()))
        .title(title)
        .inner_size(1040.0, 640.0)
        .min_inner_size(760.0, 420.0)
        .build()
        .map_err(err)?;
    Ok(())
}

/// Files dropped on the session window: the file window uploads them, so their
/// progress shows there. It picks them up with `take_drops` when it opens or is told.
#[tauri::command]
fn queue_drop(app: AppHandle, state: State<AppState>, session: u32, paths: Vec<String>) -> CmdResult<()> {
    state.drops.lock().unwrap().entry(session).or_default().extend(paths);
    open_files(app.clone(), state, session)?;
    let _ = app.emit_to(format!("files-{session}"), "files-drop", session);
    Ok(())
}

#[tauri::command]
fn take_drops(state: State<AppState>, session: u32) -> Vec<String> {
    state.drops.lock().unwrap().remove(&session).unwrap_or_default()
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
            save_settings,
            forget_peer,
            set_alias,
            connect,
            attach,
            send_input,
            select_display,
            request_keyframe,
            send_sas,
            lock_screen,
            restart_host,
            set_quality,
            disconnect,
            end_hosted_session,
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
                    if matches!(event, ctxremote_core::host::HostEvent::SessionStarted { .. }) {
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
    let commands: fn(tauri::ipc::Invoke) -> bool = handlers![];
    #[cfg(feature = "quick")]
    let builder = builder.manage(quick::Approvals::default());

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
            });

            #[cfg(not(feature = "quick"))]
            build_tray(app)?;

            show_main(app.handle());
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
