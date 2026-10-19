//! The account's team in the app (see `docs/ACCOUNTS.md`, "Teams"): kept in
//! step with the account sync, shown in the account panel, and the access
//! without a password for the team's members.
//!
//! Each sync stores the current team key and the devices open to the team
//! in the config, renews this computer's grant after a new epoch and keeps
//! this computer in its account's access list.

use std::sync::Mutex;

use ctxremote_core::proto::account::{AccountError, TeamRole};
use ctxremote_core::proto::DeviceId;
use ctxremote_core::team::{self, AccessList, TeamAccessGrant, TeamView};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::account::Sync;
use crate::{err, AppState, CmdResult};

/// The last team seen, for the panel without asking the server.
#[derive(Default)]
pub struct TeamCache {
    info: Mutex<Option<TeamInfo>>,
    error: Mutex<Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamInfo {
    name: String,
    role: TeamRole,
    me: u64,
    epoch: u32,
    members: Vec<MemberInfo>,
    devices: Vec<DeviceInfo>,
    /// This computer lets the team's members in without a password.
    access: bool,
    /// Why the last team sync failed, if it did.
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberInfo {
    id: u64,
    role: TeamRole,
    email: Option<String>,
    /// The team key vouches for its key; false right after joining or if
    /// the server slipped it in.
    verified: bool,
    since: u64,
    me: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    id: String,
    name: String,
    /// In the shared address book (the admins' list).
    in_book: bool,
    /// A member opened it to the team: connecting needs no password.
    open: bool,
    /// Who opened it (address or "Konto n").
    opened_by: Option<String>,
}

fn member_name(view: &TeamView, id: u64) -> String {
    view.members
        .iter()
        .find(|m| m.id == id)
        .and_then(|m| m.email.clone())
        .unwrap_or_else(|| format!("Konto {id}"))
}

fn info(view: &TeamView, access: bool, error: Option<String>) -> TeamInfo {
    let members = view
        .members
        .iter()
        .map(|m| MemberInfo {
            id: m.id,
            role: m.role,
            email: m.email.clone(),
            verified: !view.unverified.contains(&m.id),
            since: m.since,
            me: m.id == view.link.me,
        })
        .collect();
    let mut devices: Vec<DeviceInfo> = view
        .book
        .book
        .entries
        .iter()
        .map(|(id, entry)| {
            let opened = view.access.get(id);
            DeviceInfo {
                id: id.to_string(),
                name: entry.alias.clone().unwrap_or_else(|| entry.name.clone()),
                in_book: true,
                open: opened.is_some(),
                opened_by: opened.map(|(member, _)| member_name(view, *member)),
            }
        })
        .collect();
    for (id, (member, name)) in &view.access {
        if !view.book.book.entries.contains_key(id) {
            devices.push(DeviceInfo {
                id: id.to_string(),
                name: name.clone(),
                in_book: false,
                open: true,
                opened_by: Some(member_name(view, *member)),
            });
        }
    }
    devices.sort_by_key(|d| d.name.to_lowercase());
    TeamInfo {
        name: view.book.name.clone(),
        role: view.link.role,
        me: view.link.me,
        epoch: view.link.epoch,
        members,
        devices,
        access,
        error,
    }
}

/// The ID this computer is reached under: the app's own, or the service's.
fn this_host(state: &AppState) -> Option<DeviceId> {
    use ctxremote_core::host::Presence;
    let presence = match &state.host {
        crate::Side::Local(host) => host.presence().borrow().clone(),
        crate::Side::Service(service) => service.state().presence,
    };
    match presence {
        Presence::Online { id } => id.parse().ok(),
        _ => None,
    }
}

/// The team and epoch this computer's grant belongs to, if it has one.
fn granted(state: &AppState) -> Option<(String, u32)> {
    match &state.host {
        crate::Side::Local(_) => {
            let config = state.config.read().unwrap();
            config.team_access.as_ref().map(|g| (g.team.clone(), g.epoch))
        }
        crate::Side::Service(service) => service.state().team_access,
    }
}

fn credentials(state: &AppState) -> CmdResult<(String, ed25519_dalek::SigningKey, ctxremote_core::account::AccountLink)> {
    let config = state.config.read().unwrap();
    let link = config.account.clone().ok_or("Erst ein Konto anlegen oder anmelden")?;
    Ok((config.server_addr(), config.signing_key().map_err(err)?, link))
}

fn gone(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<AccountError>(), Some(AccountError::NoTeam | AccountError::NotLinked))
}

/// Brings the team up to date; called by the account sync. Never fails
/// loudly: the error is shown in the panel.
pub async fn sync(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (server, key, link, cached) = {
        let config = state.config.read().unwrap();
        let Some(link) = config.account.clone() else {
            drop(config);
            cleared(app);
            return;
        };
        let Ok(key) = config.signing_key() else { return };
        (config.server_addr(), key, link, config.team.clone())
    };
    match team::sync(&server, &key, &link, cached.as_ref()).await {
        Ok(Some(view)) => applied(app, view).await,
        Ok(None) => cleared(app),
        Err(e) if gone(&e) => cleared(app),
        Err(e) => {
            tracing::info!("Team nicht abgeglichen: {e:#}");
            let cache = app.state::<TeamCache>();
            *cache.error.lock().unwrap() = Some(format!("{e:#}"));
            let mut info = cache.info.lock().unwrap();
            if let Some(info) = info.as_mut() {
                info.error = Some(format!("{e:#}"));
            }
        }
    }
}

/// Stores what the sync brought and does the follow-ups.
async fn applied(app: &AppHandle, view: TeamView) {
    let state = app.state::<AppState>();
    {
        let mut config = state.config.write().unwrap();
        config.team = Some(view.link.clone());
        config.team_open = view.access.iter().map(|(id, (_, name))| (*id, name.clone())).collect();
        let _ = config.save();
    }
    renew(app, &view);
    let open = granted(&state).is_some_and(|(team, _)| team == view.link.id);
    if let Err(e) = publish_access(app, &view, open).await {
        tracing::info!("Freigabe für das Team nicht gemeldet: {e:#}");
    }
    let cache = app.state::<TeamCache>();
    *cache.error.lock().unwrap() = None;
    *cache.info.lock().unwrap() = Some(info(&view, open, None));
    let _ = app.emit("team-changed", ());
}

/// After a new epoch, this computer's grant gets the new password; the
/// service takes that without elevation (see `TeamAccessGrant::renewed_by`).
fn renew(app: &AppHandle, view: &TeamView) {
    let state = app.state::<AppState>();
    let Some((team, epoch)) = granted(&state) else { return };
    if team != view.link.id || epoch >= view.link.epoch {
        return;
    }
    let Some(host) = this_host(&state) else { return };
    let Ok(grant) = TeamAccessGrant::for_host(&state.config.read().unwrap(), host) else { return };
    match &state.host {
        crate::Side::Local(_) => {
            let mut config = state.config.write().unwrap();
            config.team_access = Some(grant);
            let _ = config.save();
        }
        crate::Side::Service(service) => service.renew_team_access(grant),
    }
    tracing::info!(epoch = view.link.epoch, "Freigabe für das Team erneuert");
}

/// Keeps this computer in (or out of) its account's access list, leaving
/// the entries of the account's other devices alone.
async fn publish_access(app: &AppHandle, view: &TeamView, open: bool) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let Some(host) = this_host(&state) else { return Ok(()) };
    let mut list = AccessList::default();
    for (id, (member, name)) in &view.access {
        if *member == view.link.me {
            list.hosts.insert(*id, (name.clone(), 0));
        }
    }
    let listed = list.hosts.contains_key(&host.get());
    if listed == open {
        return Ok(());
    }
    if open {
        list.hosts.insert(host.get(), (ctxremote_core::account::default_label(), team::stamp()));
    } else {
        list.hosts.remove(&host.get());
    }
    let (server, key, link) = credentials(&state).map_err(anyhow::Error::msg)?;
    team::set_access(&server, &key, &link, &list).await
}

/// No team (left, removed, never joined): forget its key and devices.
fn cleared(app: &AppHandle) {
    let state = app.state::<AppState>();
    {
        let mut config = state.config.write().unwrap();
        if config.team.is_none() && config.team_open.is_empty() && config.team_access.is_none() {
            drop(config);
            *app.state::<TeamCache>().info.lock().unwrap() = None;
            return;
        }
        config.team = None;
        config.team_open.clear();
        // The membership check refuses everyone now anyway; the service's
        // grant needs an administrator to go, the app's own does not.
        config.team_access = None;
        let _ = config.save();
    }
    *app.state::<TeamCache>().info.lock().unwrap() = None;
    let _ = app.emit("team-changed", ());
}

/// Syncs the team now and returns it.
async fn refreshed(app: &AppHandle) -> Option<TeamInfo> {
    sync(app).await;
    app.state::<TeamCache>().info.lock().unwrap().clone()
}

/// The team as last seen; `None` without one.
#[tauri::command]
pub fn team_info(cache: State<'_, TeamCache>) -> Option<TeamInfo> {
    let mut info = cache.info.lock().unwrap().clone()?;
    info.error = cache.error.lock().unwrap().clone();
    Some(info)
}

/// Syncs the team right away, e.g. when the panel opens.
#[tauri::command]
pub async fn team_refresh(app: AppHandle, sync: State<'_, Sync>) -> CmdResult<Option<TeamInfo>> {
    let _turn = sync.turn().lock().await;
    Ok(refreshed(&app).await)
}

#[tauri::command]
pub async fn team_create(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, name: String) -> CmdResult<Option<TeamInfo>> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Bitte einen Namen für das Team angeben".into());
    }
    let _turn = sync.turn().lock().await;
    let (server, key, link) = credentials(&state)?;
    team::create(&server, &key, &link, &name).await.map_err(err)?;
    Ok(refreshed(&app).await)
}

fn role(text: &str) -> CmdResult<TeamRole> {
    match text {
        "Owner" => Ok(TeamRole::Owner),
        "Admin" => Ok(TeamRole::Admin),
        "Technician" => Ok(TeamRole::Technician),
        _ => Err("Unbekannte Rolle".into()),
    }
}

/// A code for someone to join with `role` ("Admin" or "Technician").
#[tauri::command]
pub async fn team_invite(state: State<'_, AppState>, role: String) -> CmdResult<String> {
    let role = self::role(&role)?;
    let (server, key, link) = credentials(&state)?;
    team::invite(&server, &key, &link, role).await.map_err(err)
}

#[tauri::command]
pub async fn team_join(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, code: String) -> CmdResult<Option<TeamInfo>> {
    let _turn = sync.turn().lock().await;
    let (server, key, link) = credentials(&state)?;
    let view = team::join(&server, &key, &link, &code).await.map_err(err)?;
    // Kept right away: should the sync below fail, the next one still finds the key.
    {
        let mut config = state.config.write().unwrap();
        config.team = Some(view.link.clone());
        let _ = config.save();
    }
    Ok(refreshed(&app).await)
}

/// Changes a member's role; "Owner" hands the team over.
#[tauri::command]
pub async fn team_set_role(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, member: u64, role: String) -> CmdResult<Option<TeamInfo>> {
    let role = self::role(&role)?;
    let _turn = sync.turn().lock().await;
    let (server, key, _) = credentials(&state)?;
    team::set_role(&server, &key, member, role).await.map_err(err)?;
    Ok(refreshed(&app).await)
}

/// Removes a member; the team moves to a new key at once.
#[tauri::command]
pub async fn team_remove(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, member: u64) -> CmdResult<Option<TeamInfo>> {
    let _turn = sync.turn().lock().await;
    let (server, key, link) = credentials(&state)?;
    team::rotate(&server, &key, &link, &[member]).await.map_err(err)?;
    Ok(refreshed(&app).await)
}

#[tauri::command]
pub async fn team_leave(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>) -> CmdResult<()> {
    let _turn = sync.turn().lock().await;
    let (server, key, _) = credentials(&state)?;
    team::leave(&server, &key).await.map_err(err)?;
    cleared(&app);
    Ok(())
}

#[tauri::command]
pub async fn team_delete(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>) -> CmdResult<()> {
    let _turn = sync.turn().lock().await;
    let (server, key, _) = credentials(&state)?;
    team::delete(&server, &key).await.map_err(err)?;
    cleared(&app);
    Ok(())
}

#[tauri::command]
pub async fn team_rename(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, name: String) -> CmdResult<Option<TeamInfo>> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Bitte einen Namen für das Team angeben".into());
    }
    let _turn = sync.turn().lock().await;
    let (server, key, link) = credentials(&state)?;
    team::update_book(&server, &key, &link, |book| book.name = name.clone()).await.map_err(err)?;
    Ok(refreshed(&app).await)
}

/// Adds a device to the shared address book, or renames it there.
#[tauri::command]
pub async fn team_add_device(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, id: String, name: String) -> CmdResult<Option<TeamInfo>> {
    let device: DeviceId = id.trim().replace(' ', "").parse().map_err(|_| "Unbekannte Geräte-ID".to_string())?;
    let name = name.trim().to_string();
    let _turn = sync.turn().lock().await;
    let (server, key, link) = credentials(&state)?;
    let now = ctxremote_core::account::now_ms();
    team::update_book(&server, &key, &link, |book| {
        let entry = book.book.entries.entry(device.get()).or_default();
        entry.alias = (!name.is_empty()).then(|| name.clone());
        entry.alias_at = now;
        book.book.removed.remove(&device.get());
    })
    .await
    .map_err(err)?;
    Ok(refreshed(&app).await)
}

#[tauri::command]
pub async fn team_remove_device(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, id: String) -> CmdResult<Option<TeamInfo>> {
    let device: DeviceId = id.trim().replace(' ', "").parse().map_err(|_| "Unbekannte Geräte-ID".to_string())?;
    let _turn = sync.turn().lock().await;
    let (server, key, link) = credentials(&state)?;
    let now = ctxremote_core::account::now_ms();
    team::update_book(&server, &key, &link, |book| {
        book.book.entries.remove(&device.get());
        book.book.removed.insert(device.get(), now);
    })
    .await
    .map_err(err)?;
    Ok(refreshed(&app).await)
}

/// Lets the team's members connect to this computer without a password, or
/// no longer. With the service this needs administrator rights (UAC).
#[tauri::command]
pub async fn team_set_access(app: AppHandle, state: State<'_, AppState>, sync: State<'_, Sync>, enabled: bool) -> CmdResult<Option<TeamInfo>> {
    let _turn = sync.turn().lock().await;
    let host = this_host(&state).ok_or("Das Gerät ist gerade nicht mit dem Server verbunden")?;
    let grant = if enabled {
        Some(TeamAccessGrant::for_host(&state.config.read().unwrap(), host).map_err(err)?)
    } else {
        None
    };
    match &state.host {
        crate::Side::Local(_) => {
            let mut config = state.config.write().unwrap();
            config.team_access = grant;
            config.save().map_err(err)?;
        }
        crate::Side::Service(_) => crate::service::configure_team_access(grant).await?,
    }
    Ok(refreshed(&app).await)
}
