#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "quick")]
mod quick;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use ctxremote_core::config::Config;
use ctxremote_core::host::{Host, Presence};
use ctxremote_core::proto::session::{HostInfo, InputEvent, VideoFrame, ViewerMsg};
use ctxremote_core::proto::DeviceId;
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
    host: Host,
    viewers: Mutex<HashMap<u32, Viewer>>,
    next_viewer: AtomicU32,
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
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Binary messages to session windows: a 12-byte header, then the payload.
const PACKET_VIDEO: u8 = 1;
const PACKET_CLOSED: u8 = 2;

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
    Overview {
        presence: state.host.presence().borrow().clone(),
        password: state.host.password(),
        server: config.server.clone(),
        unattended: config.permanent_password.as_deref().is_some_and(|p| !p.is_empty()),
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
        hosted: state
            .host
            .sessions()
            .into_iter()
            .map(|(session, peer)| Hosted { session, peer })
            .collect(),
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
fn refresh_password(state: State<AppState>) -> String {
    state.host.refresh_password();
    state.host.password()
}

/// `permanent_password`: `None` keeps the current one, `Some("")` disables unattended access.
#[tauri::command]
fn save_settings(
    state: State<AppState>,
    server: String,
    permanent_password: Option<String>,
) -> CmdResult<()> {
    let server_changed = {
        let mut config = state.config.write().unwrap();
        let server = server.trim().to_string();
        if server.is_empty() {
            return Err("Bitte eine Serveradresse angeben".into());
        }
        let changed = config.server != server;
        config.server = server;
        if let Some(password) = permanent_password {
            if !password.is_empty() && password.chars().count() < 8 {
                return Err("Das Passwort braucht mindestens 8 Zeichen".into());
            }
            config.permanent_password = Some(password).filter(|p| !p.is_empty());
        }
        config.save().map_err(err)?;
        changed
    };
    if server_changed {
        state.host.reconnect();
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
fn send_sas(state: State<AppState>, session: u32) {
    with_viewer(&state, session, |s| s.send(ViewerMsg::SecureAttention));
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
            disconnect,
            end_hosted_session,
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
            let host = tauri::async_runtime::block_on(async { Host::start(config.clone()) });
            #[cfg(feature = "quick")]
            host.require_approval(quick::approver(app.handle().clone()));

            let handle = app.handle().clone();
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

            let handle = app.handle().clone();
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

            app.manage(AppState {
                config,
                host,
                viewers: Mutex::default(),
                next_viewer: AtomicU32::new(1),
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
