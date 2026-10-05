//! Service mode: the app only shows what the installed CTXRemote service hosts.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctxremote_core::config::DirectSettings;
use ctxremote_core::host::{HostEvent, Presence};
use ctxremote_core::ui_link::{ServiceLink, ServiceState, UiEvent, UiRequest};
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, watch};

const RECONNECT_EVERY: Duration = Duration::from_secs(5);
const GONE: &str = "CTXRemote-Dienst nicht erreichbar";

pub struct Service {
    link: Mutex<Option<ServiceLink>>,
    state: watch::Sender<ServiceState>,
    /// Waits for the service's answer to `SetPublicAlias`.
    alias_answer: Mutex<Option<oneshot::Sender<Result<Option<String>, String>>>>,
}

impl Service {
    /// Tries once to reach the service; `None` means there is none (local host mode).
    pub async fn detect(app: &AppHandle) -> Option<Arc<Service>> {
        let (state, _) = watch::channel(ServiceState {
            presence: Presence::Connecting,
            password: String::new(),
            server: String::new(),
            unattended: false,
            sessions: Vec::new(),
            session_profiles: Vec::new(),
            direct: DirectSettings { enabled: false, port: 0, addresses: Vec::new() },
            direct_active: false,
            chat_sessions: Vec::new(),
            public_alias: None,
        });
        let service = Arc::new(Service { link: Mutex::new(None), state, alias_answer: Mutex::new(None) });
        match tokio::time::timeout(Duration::from_secs(2), establish(service.clone(), app.clone()))
            .await
        {
            Ok(Ok(())) => Some(service),
            Ok(Err(e)) => {
                tracing::info!("Kein CTXRemote-Dienst: {e:#}");
                None
            }
            Err(_) => {
                tracing::info!("Kein CTXRemote-Dienst: Zeitüberschreitung");
                None
            }
        }
    }

    pub fn state(&self) -> ServiceState {
        self.state.borrow().clone()
    }

    pub fn send(&self, request: UiRequest) {
        if let Some(link) = &*self.link.lock().unwrap() {
            link.send(request);
        }
    }

    /// Asks the service to set the public alias and waits for its answer.
    pub async fn set_public_alias(&self, alias: Option<String>) -> Result<Option<String>, String> {
        let (tx, rx) = oneshot::channel();
        *self.alias_answer.lock().unwrap() = Some(tx);
        self.send(UiRequest::SetPublicAlias(alias));
        match tokio::time::timeout(Duration::from_secs(25), rx).await {
            Ok(Ok(answer)) => answer,
            _ => Err("Der Dienst hat nicht geantwortet".into()),
        }
    }

    /// Asks for a new one-time password and waits for the service to report it.
    pub async fn refresh_password(&self) -> String {
        let mut rx = self.state.subscribe();
        let old = rx.borrow_and_update().password.clone();
        self.send(UiRequest::RefreshPassword);
        let wait = rx.wait_for(|s| s.password != old);
        let changed = tokio::time::timeout(Duration::from_secs(3), wait)
            .await
            .ok()
            .and_then(|r| r.ok().map(|s| s.password.clone()));
        changed.unwrap_or_else(|| self.state().password)
    }
}

/// Boxed so the reconnect loop may call it from its own callback.
fn establish(
    service: Arc<Service>,
    app: AppHandle,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
    Box::pin(async move {
        let on_event = {
            let service = service.clone();
            let app = app.clone();
            move |event: Option<UiEvent>| match event {
                Some(UiEvent::State(state)) => {
                    let _ = app.emit("presence", state.presence.clone());
                    service.state.send_replace(state);
                }
                Some(UiEvent::Host(event)) => {
                    if matches!(event, HostEvent::SessionStarted { .. } | HostEvent::Chat { .. }) {
                        crate::show_main(&app);
                    }
                    let _ = app.emit("host-event", event);
                }
                Some(UiEvent::Configured(_)) => {}
                Some(UiEvent::AliasSet(answer)) => {
                    if let Some(tx) = service.alias_answer.lock().unwrap().take() {
                        let _ = tx.send(answer);
                    }
                }
                None => {
                    *service.link.lock().unwrap() = None;
                    let presence = Presence::Offline { reason: GONE.into() };
                    let _ = app.emit("presence", presence.clone());
                    service.state.send_modify(|s| s.presence = presence);
                    reconnect(service.clone(), app.clone());
                }
            }
        };
        let link = ServiceLink::connect(on_event).await?;
        *service.link.lock().unwrap() = Some(link);
        Ok(())
    })
}

fn reconnect(service: Arc<Service>, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RECONNECT_EVERY).await;
            if establish(service.clone(), app.clone()).await.is_ok() {
                break;
            }
        }
    });
}

#[derive(serde::Serialize)]
struct Request<'a> {
    server: &'a str,
    permanent_password: Option<&'a str>,
    /// When set, the helper only changes the direct connection (and the firewall rule).
    direct: Option<&'a DirectSettings>,
}

/// Applies settings through the service executable, elevated via UAC.
pub async fn configure(server: String, permanent_password: Option<String>) -> Result<(), String> {
    elevated(move |file, exe| run_elevated(exe, file, &server, permanent_password.as_deref(), None)).await
}

/// Applies the direct-connection settings the same way.
pub async fn configure_direct(settings: DirectSettings) -> Result<(), String> {
    elevated(move |file, exe| run_elevated(exe, file, "", None, Some(&settings))).await
}

async fn elevated(
    run: impl FnOnce(&std::path::Path, &std::path::Path) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let exe = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("ctxremote-service.exe");
        if !exe.is_file() {
            return Err("ctxremote-service.exe fehlt neben der App. Bitte CTXRemote neu installieren.".to_string());
        }
        let file = request_file()?;
        let result = run(&file, &exe);
        let _ = std::fs::remove_file(&file);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A new file with an unguessable name in the user's temp directory.
fn request_file() -> Result<PathBuf, String> {
    use std::hash::{BuildHasher, Hasher};
    for _ in 0..8 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let path = std::env::temp_dir().join(format!("ctxremote-{:016x}.json", hasher.finish()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Temporäre Datei nicht anlegbar: {e}")),
        }
    }
    Err("Temporäre Datei nicht anlegbar".into())
}

fn run_elevated(
    exe: &std::path::Path,
    file: &std::path::Path,
    server: &str,
    permanent_password: Option<&str>,
    direct: Option<&DirectSettings>,
) -> Result<(), String> {
    let request = serde_json::to_vec(&Request { server, permanent_password, direct })
        .map_err(|e| e.to_string())?;
    std::fs::write(file, &request).map_err(|e| format!("Temporäre Datei nicht beschreibbar: {e}"))?;
    // The file is writable by any of the user's processes; the elevated helper
    // only accepts it if it still matches what we wrote.
    use sha2::Digest;
    let digest = hex::encode(sha2::Sha256::digest(&request));
    launch(exe, file, &digest)?;

    let answer = std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    match answer {
        Some(v) if v.get("ok").and_then(|o| o.as_bool()) == Some(true) => Ok(()),
        Some(v) if v.get("error").is_some() => Err(v["error"]
            .as_str()
            .unwrap_or("Unbekannter Fehler")
            .to_string()),
        _ => Err("Der Dienst hat die Einstellungen nicht bestätigt".into()),
    }
}

#[cfg(windows)]
fn launch(exe: &std::path::Path, file: &std::path::Path, digest: &str) -> Result<(), String> {
    use windows::core::{w, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect()
    }
    let exe_w = wide(exe.as_os_str());
    let mut args = std::ffi::OsString::from("--configure \"");
    args.push(file);
    args.push("\" ");
    args.push(digest);
    let args_w = wide(&args);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                "Abgebrochen – für diese Einstellung sind Administratorrechte nötig".into()
            } else {
                format!("Der Dienst-Helfer konnte nicht gestartet werden: {e}")
            });
        }
        if !info.hProcess.is_invalid() {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn launch(_: &std::path::Path, _: &std::path::Path, _: &str) -> Result<(), String> {
    Err("Den CTXRemote-Dienst gibt es nur unter Windows".into())
}
