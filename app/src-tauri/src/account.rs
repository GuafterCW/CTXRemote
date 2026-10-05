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
        // The account was deleted (in the web interface) or this device was
        // removed from it: work on without one.
        Err(e) if e.downcast_ref::<ctxremote_core::proto::account::AccountError>()
            == Some(&ctxremote_core::proto::account::AccountError::NotLinked) =>
        {
            tracing::info!("Nicht mehr im Konto, Verknüpfung entfernt");
            let _ = save_link(&state, None);
            *sync.error.lock().unwrap() = None;
            let _ = app.emit("peers-changed", ());
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
    linked(&app, &state, link, _turn).await
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
    linked(&app, &state, link, _turn).await
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

/// Registers an account with a login; returns the recovery code to show once.
#[tauri::command]
pub async fn account_register(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, email: String, password: String) -> CmdResult<String> {
    let turn = sync.turn.lock().await;
    let (server, key) = credentials(&state)?;
    let (link, code) = account::register(&server, &key, &email, &password).await.map_err(|e| format!("{e:#}"))?;
    linked(&app, &state, link, turn).await?;
    Ok(code)
}

/// Adds this device to the account of `email`.
#[tauri::command]
pub async fn account_login(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, email: String, password: String) -> CmdResult<()> {
    let turn = sync.turn.lock().await;
    let (server, key) = credentials(&state)?;
    let link = account::login(&server, &key, &email, &password).await.map_err(|e| format!("{e:#}"))?;
    linked(&app, &state, link, turn).await
}

/// Password forgotten: adds this device with the recovery code and sets a new
/// password. Returns the new recovery code.
#[tauri::command]
pub async fn account_recover(
    app: AppHandle,
    state: State<'_, AppState>,
    sync: State<'_, Sync>,
    email: String,
    code: String,
    password: String,
) -> CmdResult<String> {
    let turn = sync.turn.lock().await;
    let (server, key) = credentials(&state)?;
    let (link, code) = account::recover(&server, &key, &email, &code, &password).await.map_err(|e| format!("{e:#}"))?;
    linked(&app, &state, link, turn).await?;
    Ok(code)
}

/// Sets address and password of this device's account (adds a login to an
/// account made by pairing). Returns the new recovery code.
#[tauri::command]
pub async fn account_set_login(state: State<'_, AppState>, email: String, password: String) -> CmdResult<String> {
    let (server, key) = credentials(&state)?;
    let link = state.config.read().unwrap().account.clone().ok_or("Dieses Gerät gehört zu keinem Konto")?;
    account::set_login(&server, &key, &link, &email, &password).await.map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDetails {
    email: Option<String>,
    verified: bool,
    devices: Vec<account::AccountDevice>,
}

/// The account's login and devices, fresh from the server.
#[tauri::command]
pub async fn account_details(state: State<'_, AppState>) -> CmdResult<AccountDetails> {
    let (server, key) = credentials(&state)?;
    let link = state.config.read().unwrap().account.clone().ok_or("Dieses Gerät gehört zu keinem Konto")?;
    let login = account::login_status(&server, &key).await.map_err(|e| format!("{e:#}"))?;
    let devices = account::devices(&server, &key, &link).await.map_err(|e| format!("{e:#}"))?;
    Ok(AccountDetails {
        email: login.as_ref().map(|l| l.email.clone()),
        verified: login.is_some_and(|l| l.verified),
        devices,
    })
}

/// Takes another device out of the account.
#[tauri::command]
pub async fn account_remove_device(state: State<'_, AppState>, public_key: String) -> CmdResult<()> {
    let (server, key) = credentials(&state)?;
    account::remove_device(&server, &key, &public_key).await.map_err(|e| format!("{e:#}"))
}

/// Stores the link, names this device for the others and syncs the list.
async fn linked(
    app: &AppHandle,
    state: &AppState,
    link: account::AccountLink,
    turn: tokio::sync::MutexGuard<'_, ()>,
) -> CmdResult<()> {
    save_link(state, Some(link.clone()))?;
    drop(turn);
    if let Ok((server, key)) = credentials(state) {
        let _ = account::set_label(&server, &key, &link, &account::default_label()).await;
    }
    sync_now(app).await;
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
    if link.is_none() {
        // Without an account there is nothing to let in, and nothing to know.
        config.account_access = None;
        config.access.clear();
    }
    config.account = link;
    config.save().map_err(err)
}

/// Lets the account's devices connect to this computer without a password,
/// or no longer. With the service this needs administrator rights (UAC), as
/// it grants unattended access.
#[tauri::command]
pub async fn account_set_access(state: State<'_, AppState>, sync: State<'_, Sync>, enabled: bool) -> CmdResult<()> {
    use ctxremote_core::host::Presence;
    // Not while a sync writes the list back, or this choice could be lost.
    let _turn = sync.turn.lock().await;
    // The ID this computer is reached under: the app's own, or the service's.
    let host = match &state.host {
        crate::Side::Local(host) => host.presence().borrow().clone(),
        crate::Side::Service(service) => service.state().presence,
    };
    let host: ctxremote_core::proto::DeviceId = match host {
        Presence::Online { id } => id.parse().map_err(|_| "Unbekannte Geräte-ID".to_string())?,
        _ => return Err("Das Gerät ist gerade nicht mit dem Server verbunden".into()),
    };
    let grant = if enabled {
        Some(account::AccessGrant::for_host(&state.config.read().unwrap(), host).map_err(err)?)
    } else {
        None
    };
    match &state.host {
        crate::Side::Local(_) => state.config.write().unwrap().account_access = grant,
        crate::Side::Service(_) => crate::service::configure_access(grant).await?,
    }
    {
        // Tell the account's other devices, through the shared list.
        let mut config = state.config.write().unwrap();
        config.access.insert(host.get(), account::Access { open: enabled, at: account::now_ms() });
        config.save().map_err(err)?;
    }
    sync.poke();
    Ok(())
}
