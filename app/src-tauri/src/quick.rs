//! "CTXRemote Hilfe": the portable one-shot build. Config is throwaway and every
//! incoming connection has to be confirmed by the user.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ctxremote_core::config::Config;
use ctxremote_core::host::Approver;
use ctxremote_core::profile::Profile;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State, UserAttentionType};
use tokio::sync::oneshot;

/// Server address baked in at build time; a missing variable fails the build
/// (const panic, since `compile_error!` cannot depend on the environment).
const SERVER: &str = match option_env!("CTXREMOTE_QUICK_SERVER") {
    Some(server) => server,
    None => panic!(
        "Für den Quick-Build muss CTXREMOTE_QUICK_SERVER gesetzt sein, z. B. \"remote.example.org:21300\""
    ),
};

#[derive(Default)]
pub struct Approvals {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
}

fn config_path() -> PathBuf {
    std::env::temp_dir().join(format!("ctxremote-hilfe-{}.json", std::process::id()))
}

/// Pins the throwaway config file; must run before the config is first read.
pub fn pin_config() {
    Config::use_path(config_path());
}

/// Fresh config pointing at the built-in server, without a permanent password.
pub fn load_config() -> anyhow::Result<Config> {
    let mut config = Config::load()?;
    config.server = SERVER.to_string();
    config.permanent_password = None;
    // A portable helper should not open a port (and trigger a firewall prompt).
    config.direct = false;
    Ok(config)
}

/// Best effort; a hard-killed process leaves its file behind.
pub fn cleanup() {
    let _ = std::fs::remove_file(config_path());
}

/// Tells the frontend to drop the dialog, also when the host gave up waiting.
struct Closer {
    app: AppHandle,
    id: u64,
}

impl Drop for Closer {
    fn drop(&mut self) {
        self.app.state::<Approvals>().pending.lock().unwrap().remove(&self.id);
        let _ = self.app.emit("approval-closed", json!({ "id": self.id }));
    }
}

pub fn approver(app: AppHandle) -> Approver {
    std::sync::Arc::new(move |peer: String, profile: Option<Profile>| {
        let app = app.clone();
        Box::pin(async move {
            let approvals = app.state::<Approvals>();
            let id = approvals.next.fetch_add(1, Ordering::Relaxed) + 1;
            let (tx, rx) = oneshot::channel();
            approvals.pending.lock().unwrap().insert(id, tx);
            let _closer = Closer { app: app.clone(), id };

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.request_user_attention(Some(UserAttentionType::Critical));
            }
            let _ = app.emit("approval-request", json!({ "id": id, "peer": peer, "profile": profile }));
            rx.await.unwrap_or(false)
        })
    })
}

#[tauri::command]
pub fn answer_approval(approvals: State<Approvals>, id: u64, allow: bool) {
    if let Some(tx) = approvals.pending.lock().unwrap().remove(&id) {
        let _ = tx.send(allow);
    }
}
