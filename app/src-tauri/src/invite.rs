//! Invitation links (see `ctxremote_proto::invite`).
//!
//! The app hands out links with a random token and keeps the tokens in its
//! own folder; an offer that comes back is opened here, never by the service
//! or the server. The quick helper finds its inviter in its own file name.

use ctxremote_core::proto::invite;
use ctxremote_core::proto::DeviceId;
#[cfg(not(feature = "quick"))]
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(not(feature = "quick"))]
use tauri::Emitter;
#[cfg(not(feature = "quick"))]
use tauri::Manager;
#[cfg(not(feature = "quick"))]
use tauri::AppHandle;
#[cfg(feature = "quick")]
use tauri::State;

#[cfg(feature = "quick")]
use crate::{AppState, Side};
use crate::CmdResult;

/// Where the download page for invited people lives.
#[cfg(not(feature = "quick"))]
const WEBSITE: &str = match option_env!("CTXREMOTE_WEBSITE") {
    Some(site) => site,
    None => "https://remocto.com",
};

/// How long a link works.
#[cfg(not(feature = "quick"))]
const VALID_SECS: u64 = 7 * 24 * 3600;
/// Links kept at most; the oldest go first.
#[cfg(not(feature = "quick"))]
const KEPT: usize = 20;

#[cfg(not(feature = "quick"))]
#[derive(Serialize, Deserialize, Clone)]
struct Invite {
    token: String,
    created: u64,
}

#[cfg(not(feature = "quick"))]
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(not(feature = "quick"))]
fn store(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("invites.json"))
}

/// The links still valid.
#[cfg(not(feature = "quick"))]
fn load(app: &AppHandle) -> Vec<Invite> {
    let list: Vec<Invite> = store(app)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let now = now();
    list.into_iter().filter(|i| now.saturating_sub(i.created) < VALID_SECS && invite::valid_token(&i.token)).collect()
}

/// A new link for this device: whoever opens it gets a quick helper that
/// can send its ID and password here.
#[cfg(not(feature = "quick"))]
#[tauri::command]
pub fn create_invite(app: AppHandle, id: String) -> CmdResult<String> {
    let id: DeviceId = id.replace(' ', "").parse().map_err(|_| "Noch keine ID vom Server".to_string())?;
    let token = invite::new_token();
    let mut list = load(&app);
    list.push(Invite { token: token.clone(), created: now() });
    let skip = list.len().saturating_sub(KEPT);
    let list = &list[skip..];
    let path = store(&app).ok_or("Kein Ordner für Einstellungen")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_vec(list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    // The token goes after "#": browsers never send that part to the server.
    Ok(format!("{WEBSITE}/hilfe.html#{}-{token}", id.get()))
}

/// An offer from someone this device invited: shows it if one of our tokens
/// opens it, and drops it silently otherwise.
#[cfg(not(feature = "quick"))]
pub fn on_request(app: &AppHandle, from: &str, hint: [u8; 4], sealed: &[u8]) {
    let Ok(from) = from.parse::<DeviceId>() else { return };
    let offer = load(app)
        .iter()
        .filter(|i| invite::hint(&i.token) == hint)
        .find_map(|i| invite::open(&i.token, from, sealed));
    let Some(offer) = offer else {
        tracing::debug!(%from, "Hilfe-Anfrage ohne passenden Einladungslink verworfen");
        return;
    };
    crate::show_main(app);
    let _ = app.emit(
        "help-request",
        json!({ "id": offer.from.to_string(), "password": offer.password, "name": offer.name }),
    );
}

/// Quick helper: who invited it, from the file name the download page gave it.
#[cfg(feature = "quick")]
fn inviter() -> Option<(DeviceId, String)> {
    let exe = std::env::current_exe().ok()?;
    invite::from_file_name(&exe.file_name()?.to_string_lossy())
}

/// The inviting device's ID (formatted), if this helper came through a link.
#[cfg(feature = "quick")]
#[tauri::command]
pub fn invitation() -> Option<serde_json::Value> {
    inviter().map(|(id, _)| json!({ "id": id.to_string() }))
}

/// Quick helper: sends ID and password to whoever invited it.
#[cfg(feature = "quick")]
#[tauri::command]
pub async fn send_help(state: State<'_, AppState>) -> CmdResult<()> {
    let (id, token) = inviter().ok_or("Diese Schnellhilfe kam nicht über einen Einladungslink")?;
    let Side::Local(host) = &state.host;
    let name = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    host.offer_help(id, &token, &name).await
}
