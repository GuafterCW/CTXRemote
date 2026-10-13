//! Updates of the Microsoft Store package. Windows does not replace a package
//! while one of its programs runs, and the app runs in the tray all day. So the
//! app looks for an update itself and installs it silently while nobody uses
//! it; Windows closes the app and the service for that and starts them again.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::{AppState, Side};

/// First look shortly after start, then a few times a day.
const FIRST_CHECK: Duration = Duration::from_secs(5 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// While an update waits for a quiet moment.
const IDLE_POLL: Duration = Duration::from_secs(10 * 60);

pub fn watch(app: &AppHandle) {
    if !ctxremote_core::update::packaged() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            match tauri::async_runtime::spawn_blocking(pending).await {
                Ok(Ok(true)) => {
                    tracing::info!("Store-Update verfügbar");
                    while !idle(&app) {
                        tokio::time::sleep(IDLE_POLL).await;
                    }
                    match tauri::async_runtime::spawn_blocking(install).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::info!("Store-Update nicht installiert: {e:#}"),
                        Err(e) => tracing::info!("Store-Update nicht installiert: {e}"),
                    }
                }
                Ok(Ok(false)) => {}
                Ok(Err(e)) => tracing::info!("Store-Update-Prüfung fehlgeschlagen: {e:#}"),
                Err(e) => tracing::info!("Store-Update-Prüfung fehlgeschlagen: {e}"),
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Nobody would lose anything: no session in either direction and the main
/// window closed (it may hold half-typed settings).
fn idle(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    if !state.viewers.lock().unwrap().is_empty() {
        return false;
    }
    let hosting = match &state.host {
        Side::Local(host) => !host.sessions().is_empty(),
        Side::Service(service) => !service.state().sessions.is_empty(),
    };
    let window_open = app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false));
    !hosting && !window_open
}

#[cfg(windows)]
fn pending() -> anyhow::Result<bool> {
    use windows::Services::Store::StoreContext;
    let context = StoreContext::GetDefault()?;
    let updates = context.GetAppAndOptionalStorePackageUpdatesAsync()?.join()?;
    Ok(updates.Size()? > 0)
}

#[cfg(windows)]
fn install() -> anyhow::Result<()> {
    use windows::core::w;
    use windows::Services::Store::{StoreContext, StorePackageUpdateState};
    use windows::Win32::System::Recovery::{
        RegisterApplicationRestart, RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_REBOOT,
    };

    let context = StoreContext::GetDefault()?;
    // Ask again: the update may have been installed from the Store app meanwhile.
    let updates = context.GetAppAndOptionalStorePackageUpdatesAsync()?.join()?;
    if updates.Size()? == 0 {
        return Ok(());
    }
    if !context.CanSilentlyDownloadStorePackageUpdates()? {
        anyhow::bail!("Windows erlaubt kein stilles Update (z. B. getaktete Verbindung)");
    }
    // Windows starts the app again after it replaced the package, back into
    // the tray. Leaving out RESTART_NO_PATCH is what asks for that.
    unsafe { RegisterApplicationRestart(w!("--tray"), RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT)? };
    tracing::info!("Store-Update wird installiert, die App startet gleich neu");
    let result = context.TrySilentDownloadAndInstallStorePackageUpdatesAsync(&updates)?.join()?;
    // Only reached when Windows did not close the app.
    let state = result.OverallState()?;
    if state != StorePackageUpdateState::Completed {
        anyhow::bail!("Store meldet Zustand {}", state.0);
    }
    Ok(())
}

#[cfg(not(windows))]
fn pending() -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(not(windows))]
fn install() -> anyhow::Result<()> {
    Ok(())
}
