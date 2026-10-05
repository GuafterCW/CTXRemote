//! The account and address-book sync of the app (see `docs/ACCOUNTS.md`).
//!
//! The address book is the user's own list (`Config::peers`), so it syncs
//! from the app, also in service mode. A background task syncs at start,
//! shortly after every change to the list, and every few minutes.

use std::sync::Mutex;
use std::time::Duration;

use ctxremote_core::account::{self, Book};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::{err, AppState, CmdResult};

const EVERY: Duration = Duration::from_secs(5 * 60);
/// Changes in quick succession (renaming, removing) sync once.
const SETTLE: Duration = Duration::from_millis(800);

#[derive(Default)]
pub struct Sync {
    wake: Notify,
    /// The last sync's error, shown in the settings; cleared by a success.
    error: Mutex<Option<String>>,
    /// Serialises account changes with the background sync.
    turn: tokio::sync::Mutex<()>,
}

impl Sync {
    /// Asks for a sync soon, e.g. after the list changed.
    pub fn poke(&self) {
        self.wake.notify_one();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    devices: u32,
    /// Why the last sync failed, if it did.
    error: Option<String>,
}

pub fn view(app: &AppHandle) -> Option<AccountView> {
    let link = app.state::<AppState>().config.read().unwrap().account.clone()?;
    let error = app.state::<Sync>().error.lock().unwrap().clone();
    Some(AccountView { devices: link.devices, error })
}

/// Starts the background sync.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            sync_now(&app).await;
            let sync = app.state::<Sync>();
            let _ = tokio::time::timeout(EVERY, sync.wake.notified()).await;
            tokio::time::sleep(SETTLE).await;
        }
    });
}

/// One sync, if this user has an account. Errors are kept for the settings.
async fn sync_now(app: &AppHandle) {
    let state = app.state::<AppState>();
    let sync = app.state::<Sync>();
    let _turn = sync.turn.lock().await;
    let (server, key, link, local) = {
        let config = state.config.read().unwrap();
        let Some(link) = config.account.clone() else { return };
        let Ok(key) = config.signing_key() else { return };
        (config.server_addr(), key, link, Book::from_config(&config))
    };
    let result = account::sync(&server, &key, &link, &local).await;
    match result {
        Ok((remote, _)) => {
            *sync.error.lock().unwrap() = None;
            let changed = {
                let mut config = state.config.write().unwrap();
                let before = Book::from_config(&config);
                // Changes made while the sync ran are merged in, not lost.
                Book::merge(&before, &remote).apply_to(&mut config);
                let changed = Book::from_config(&config) != before;
                if changed {
                    let _ = config.save();
                }
                changed
            };
            if changed {
                let _ = app.emit("peers-changed", ());
            }
        }
        Err(e) => {
            tracing::info!("Adressbuch nicht abgeglichen: {e:#}");
            *sync.error.lock().unwrap() = Some(format!("{e:#}"));
        }
    }
}

/// Creates an account with this device and uploads the current list.
#[tauri::command]
pub async fn account_create(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>) -> CmdResult<()> {
    let _turn = sync.turn.lock().await;
    let (server, key) = credentials(&state)?;
    let link = account::create(&server, &key).await.map_err(|e| format!("{e:#}"))?;
    save_link(&state, Some(link))?;
    drop(_turn);
    sync.poke();
    let _ = app.emit("peers-changed", ());
    Ok(())
}

/// A one-time code for another device; valid for ten minutes.
#[tauri::command]
pub async fn account_pairing_code(state: State<'_, AppState>) -> CmdResult<String> {
    let (server, key) = credentials(&state)?;
    let link = state.config.read().unwrap().account.clone().ok_or("Dieses Gerät gehört zu keinem Konto")?;
    account::offer_pairing(&server, &key, &link).await.map_err(|e| format!("{e:#}"))
}

/// Joins the account of the device that showed `code`. The local list is
/// merged into the account's, nothing is lost.
#[tauri::command]
pub async fn account_join(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, code: String) -> CmdResult<()> {
    let _turn = sync.turn.lock().await;
    let (server, key) = credentials(&state)?;
    let link = account::join(&server, &key, &code).await.map_err(|e| format!("{e:#}"))?;
    save_link(&state, Some(link))?;
    drop(_turn);
    sync_now(&app).await;
    let _ = app.emit("peers-changed", ());
    Ok(())
}

/// Takes this device out of the account; its list stays as it is.
#[tauri::command]
pub async fn account_leave(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>) -> CmdResult<()> {
    let _turn = sync.turn.lock().await;
    let (server, key) = credentials(&state)?;
    account::leave(&server, &key).await.map_err(|e| format!("{e:#}"))?;
    save_link(&state, None)?;
    *sync.error.lock().unwrap() = None;
    let _ = app.emit("peers-changed", ());
    Ok(())
}

/// Syncs right away, e.g. when the settings open.
#[tauri::command]
pub async fn account_sync(app: AppHandle) -> CmdResult<()> {
    sync_now(&app).await;
    Ok(())
}

fn credentials(state: &AppState) -> CmdResult<(String, ed25519_dalek::SigningKey)> {
    let config = state.config.read().unwrap();
    Ok((config.server_addr(), config.signing_key().map_err(err)?))
}

fn save_link(state: &AppState, link: Option<account::AccountLink>) -> CmdResult<()> {
    let mut config = state.config.write().unwrap();
    config.account = link;
    config.save().map_err(err)
}
