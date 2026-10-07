//! Windows implementation: command line, service control and machine-wide data.

use std::ffi::OsString;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_core::config::{Config, DirectSettings};
use ctxremote_core::agent_process::{self, AgentProcess};
use ctxremote_core::host::{Host, Presence, ScreenSource};
use ctxremote_core::ui_link::{self, ServiceLink, UiEvent, UiRequest};
use sha2::{Digest, Sha256};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE,
};
use windows::Win32::Storage::FileSystem::{CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    IsWellKnownSid, SetFileSecurityW, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
    SECURITY_ATTRIBUTES,
    PSECURITY_DESCRIPTOR,
};
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};


const SERVICE_NAME: &str = "CTXRemote";
const SERVICE_DESCRIPTION: &str = "Fernzugriff auf dieses Gerät";
/// Owned by Administrators; only SYSTEM and Administrators, inherited by children, nothing from the parent.
const DATA_DIR_SDDL: &str = "O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
const ERROR_SERVICE_ALREADY_RUNNING: i32 = 1056;

const HELP: &str = "CTXRemote-Dienst

Aufruf: ctxremote-service <Option>

  --install [--server <Adresse>]
                     Dienst installieren und starten (Administrator)
  --stop             Dienst stoppen und auf das Ende warten (Administrator)
  --uninstall        Dienst stoppen und entfernen (Administrator)
  --service          Vom Dienstmanager gestartet (nicht manuell verwenden)
  --console          Host im Vordergrund ausführen, Ende mit Strg+C
  --agent <pipe>     Bildschirm-Agent starten (intern)
  --configure <datei> <sha256>
                     Einstellungen der App an den Dienst übergeben (intern, erhöht)";

/// Entry point; returns the process exit code.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--install"] => install(None),
        ["--install", "--server", server] => install(Some(server)),
        ["--stop"] => stop(),
        ["--uninstall"] => uninstall(),
        ["--service"] => service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("Start als Dienst fehlgeschlagen (nur durch den Dienstmanager möglich)"),
        ["--console"] => console(),
        ["--agent", pipe] => agent(pipe),
        ["--configure", file, digest] => configure(Path::new(file), digest),
        _ => {
            eprintln!("{HELP}");
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Fehler: {e:#}");
            1
        }
    }
}

// ---------------------------------------------------------------- data dir

fn data_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").unwrap_or_else(|| OsString::from(r"C:\ProgramData"));
    PathBuf::from(base).join("CTXRemote")
}

/// Creates the data directory and (re)applies owner and protected DACL.
///
/// Any user may create folders in ProgramData. A folder we did not create
/// could hold a planted `host.json` (with a permanent password of the
/// attacker's choosing) or be a junction elsewhere, and its owner could
/// loosen the DACL again. Such a folder is moved aside, never adopted.
fn ensure_data_dir() -> Result<PathBuf> {
    let dir = data_dir();
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) => {
            let reparse = meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
            if reparse || !meta.is_dir() || !owned_by_system_or_admins(&dir)? {
                quarantine(&dir)?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("Ordner {} nicht prüfbar", dir.display())),
    }
    let path = wide(dir.as_os_str());
    if !dir.exists() {
        // Created with its final DACL, so there is no window with inherited rights.
        // Fails if someone else created it in the meantime, which is what we want.
        with_descriptor(|descriptor| unsafe {
            let attrs = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attrs))
                .with_context(|| format!("Ordner {} nicht anlegbar", dir.display()))
        })?;
    }
    with_descriptor(|descriptor| unsafe {
        SetFileSecurityW(
            PCWSTR(path.as_ptr()),
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
        .ok()
        .context("Zugriffsrechte des Datenordners nicht setzbar")
    })?;
    std::fs::create_dir_all(dir.join("logs"))?;
    Ok(dir)
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Runs `f` with the data directory's security descriptor, freed afterwards.
fn with_descriptor<T>(f: impl FnOnce(PSECURITY_DESCRIPTOR) -> Result<T>) -> Result<T> {
    let sddl = wide(std::ffi::OsStr::new(DATA_DIR_SDDL));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the string is NUL-terminated; the descriptor is freed exactly once.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .context("Sicherheitsbeschreibung ungültig")?;
        let result = f(descriptor);
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}

fn owned_by_system_or_admins(dir: &Path) -> Result<bool> {
    let path = wide(dir.as_os_str());
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `owner` points into `descriptor`, which is freed after the last use.
    unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut descriptor,
        )
        .ok()
        .context("Besitzer des Datenordners nicht lesbar")?;
        let trusted = IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        Ok(trusted)
    }
}

/// Moves an untrusted folder (or link, or file) out of the way, keeping it for inspection.
fn quarantine(dir: &Path) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let aside = dir.with_file_name(format!("CTXRemote.untrusted-{stamp}"));
    std::fs::rename(dir, &aside).with_context(|| {
        format!("{} gehört nicht SYSTEM/Administratoren und ließ sich nicht verschieben", dir.display())
    })?;
    eprintln!(
        "Warnung: {} wurde nicht von SYSTEM oder Administratoren angelegt und nach {} verschoben.",
        dir.display(),
        aside.display()
    );
    Ok(())
}

fn init_logging(dir: &Path, file_name: &str, stderr: bool) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("logs").join(file_name))
        .context("Logdatei nicht öffnbar")?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    let file = Mutex::new(file);
    if stderr {
        builder.with_writer(file.and(std::io::stderr)).try_init()
    } else {
        builder.with_writer(file).try_init()
    }
    .map_err(|e| anyhow::anyhow!("Logging nicht initialisierbar: {e}"))
}

/// Prepares the data dir, logging and the pinned machine-wide config.
fn prepare(stderr: bool) -> Result<Config> {
    let dir = ensure_data_dir()?;
    init_logging(&dir, "service.log", stderr)?;
    Config::use_path(dir.join("host.json"));
    Config::load()
}

/// Each session's screen side runs in `<this exe> --agent <pipe>` in the console session.
fn screen_source() -> Result<Arc<dyn ScreenSource>> {
    let exe = std::env::current_exe().context("Programmpfad unbekannt")?;
    Ok(Arc::new(AgentProcess::new(exe)))
}

// ------------------------------------------------------------ run modes

/// The agent process, started by the host for one session.
fn agent(pipe: &str) -> Result<()> {
    // Best effort: the agent works without a log, too.
    let _ = init_logging(&data_dir(), "agent.log", false);
    let result = tokio::runtime::Runtime::new()?.block_on(agent_process::serve(pipe));
    if let Err(e) = &result {
        tracing::warn!("Agent beendet: {e:#}");
    }
    result
}

fn console() -> Result<()> {
    let config = prepare(true)?;
    let screen = screen_source()?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let _host = start_host(config, screen);
        tracing::info!("Host läuft im Vordergrund, Ende mit Strg+C");
        tokio::signal::ctrl_c().await
    })?;
    tracing::info!("Host beendet");
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = service_body() {
        tracing::error!("Dienst beendet mit Fehler: {e:#}");
    }
}

fn status(state: ServiceState, exit: u32) -> ServiceStatus {
    let running = state == ServiceState::Running;
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: if exit == 0 {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(exit)
        },
        checkpoint: 0,
        wait_hint: if state == ServiceState::Running || state == ServiceState::Stopped {
            Duration::default()
        } else {
            Duration::from_secs(10)
        },
        process_id: None,
    }
}

fn service_body() -> Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::StartPending, 0))?;

    let outcome = (|| -> Result<()> {
        let config = prepare(false)?;
        let runtime = tokio::runtime::Runtime::new()?;
        let _guard = runtime.enter();
        let screen = screen_source()?;
        let host = start_host(config, screen);
        tracing::info!("Dienst gestartet");
        relaunch_app_after_update();
        handle.set_service_status(status(ServiceState::Running, 0))?;
        let _ = stop_rx.recv();
        handle.set_service_status(status(ServiceState::StopPending, 0))?;
        drop(host);
        drop(_guard);
        runtime.shutdown_timeout(Duration::from_secs(5));
        tracing::info!("Dienst beendet");
        Ok(())
    })();

    let exit = u32::from(outcome.is_err());
    handle.set_service_status(status(ServiceState::Stopped, exit))?;
    outcome
}

// ------------------------------------------------------ install/uninstall

fn error_code(e: &windows_service::Error) -> Option<i32> {
    match e {
        windows_service::Error::Winapi(io) => io.raw_os_error(),
        _ => None,
    }
}

fn open_manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access).map_err(|e| {
        if error_code(&e) == Some(ERROR_ACCESS_DENIED) {
            anyhow::anyhow!(
                "Zugriff verweigert: Bitte als Administrator ausführen (Eingabeaufforderung oder PowerShell \"Als Administrator ausführen\")"
            )
        } else {
            anyhow::Error::new(e).context("Dienstverwaltung nicht erreichbar")
        }
    })
}

/// Carries the user's device identity over to the machine config, once.
fn adopt_identity(user_config: &Path, host_config: &Path) -> Result<()> {
    if host_config.exists() || !user_config.exists() {
        return Ok(());
    }
    let json = std::fs::read_to_string(user_config).context("Benutzerkonfiguration unlesbar")?;
    let user: Config = serde_json::from_str(&json).context("Benutzerkonfiguration ungültig")?;
    // The device list stays with the user; everything else moves to the service.
    let config = Config { peers: Vec::new(), ..user };
    config.save()?;
    println!("Geräte-Identität aus der Benutzerkonfiguration übernommen.");
    Ok(())
}

fn install(server: Option<&str>) -> Result<()> {
    // Must be read before the path is pinned to the machine config.
    let user_config = Config::path()?;
    let manager =
        open_manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;

    let dir = ensure_data_dir()?;
    let host_config = dir.join("host.json");
    Config::use_path(host_config.clone());
    adopt_identity(&user_config, &host_config)?;
    if let Some(server) = server {
        let mut config = Config::load()?;
        config.server = server.trim().to_string();
        config.save()?;
        println!("Server: {}", config.server);
    }

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe().context("Programmpfad unbekannt")?,
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let access = ServiceAccess::CHANGE_CONFIG
        | ServiceAccess::START
        | ServiceAccess::QUERY_STATUS
        | ServiceAccess::STOP;

    let service = match manager.open_service(SERVICE_NAME, access) {
        Ok(service) => {
            service.change_config(&info).context("Dienst nicht aktualisierbar")?;
            println!("Dienst aktualisiert.");
            service
        }
        Err(e) if error_code(&e) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => {
            let service = manager
                .create_service(&info, access)
                .context("Dienst nicht anlegbar")?;
            println!("Dienst angelegt.");
            service
        }
        Err(e) => return Err(anyhow::Error::new(e).context("Dienst nicht öffnbar")),
    };

    enable_sas_policy()?;
    start_app_at_sign_in(true);
    let config = Config::load()?;
    allow_direct_connections(config.direct, config.direct_port);
    service.set_description(SERVICE_DESCRIPTION)?;
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(5),
                };
                3
            ]),
        })
        .context("Fehleraktionen nicht setzbar")?;

    match service.start::<&str>(&[]) {
        Ok(()) => println!("Dienst gestartet."),
        Err(e) if error_code(&e) == Some(ERROR_SERVICE_ALREADY_RUNNING) => {
            println!("Dienst läuft bereits (Änderungen wirken nach einem Neustart des Dienstes).");
        }
        Err(e) => return Err(anyhow::Error::new(e).context("Dienst nicht startbar")),
    }
    Ok(())
}

/// Stops the service and waits up to 15 s for `Stopped`.
fn stop_and_wait(service: &windows_service::service::Service) -> Result<()> {
    if service.query_status()?.current_state != ServiceState::Stopped {
        // Ignore failures here: the service may already be stopping.
        let _ = service.stop();
        let deadline = Instant::now() + Duration::from_secs(15);
        while service.query_status()?.current_state != ServiceState::Stopped {
            if Instant::now() >= deadline {
                bail!("Dienst wurde nicht innerhalb von 15 Sekunden beendet");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// Opens the service; `None` if it is not installed.
fn open_existing(access: ServiceAccess) -> Result<Option<windows_service::service::Service>> {
    let manager = open_manager(ServiceManagerAccess::CONNECT)?;
    match manager.open_service(SERVICE_NAME, access) {
        Ok(service) => Ok(Some(service)),
        Err(e) if error_code(&e) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("Dienst nicht öffnbar")),
    }
}

fn stop() -> Result<()> {
    match open_existing(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)? {
        Some(service) => {
            stop_and_wait(&service)?;
            println!("Dienst gestoppt.");
        }
        None => println!("Dienst ist nicht installiert."),
    }
    Ok(())
}

fn uninstall() -> Result<()> {
    let access = ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE;
    let Some(service) = open_existing(access)? else {
        println!("Dienst ist nicht installiert.");
        return Ok(());
    };
    stop_and_wait(&service)?;
    service.delete().context("Dienst nicht löschbar")?;
    remove_firewall_rule();
    start_app_at_sign_in(false);
    println!("Dienst entfernt. Die Daten in {} bleiben erhalten.", data_dir().display());
    Ok(())
}

const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "CTXRemote";

/// With the service, the app starts in the tray at every user's sign-in, so
/// whoever sits at the computer sees running sessions and chat. Without it,
/// a session would go on unnoticed after signing out and in again.
/// Failures are reported but not fatal.
fn start_app_at_sign_in(enabled: bool) {
    let result = (|| -> Result<()> {
        let key_path = wide(std::ffi::OsStr::new(RUN_KEY));
        let name = wide(std::ffi::OsStr::new(RUN_VALUE));
        unsafe {
            let mut key = HKEY::default();
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
            .ok()
            .context("Autostart-Schlüssel nicht öffnbar")?;
            let result = if enabled {
                let exe = std::env::current_exe().context("Programmpfad unbekannt")?;
                let app = exe.with_file_name("ctxremote.exe");
                let command = wide(std::ffi::OsStr::new(&format!("\"{}\" --tray", app.display())));
                let bytes = std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2);
                RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)).ok()
            } else {
                // Already gone is fine.
                let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
                Ok(())
            };
            let _ = RegCloseKey(key);
            result.context("Autostart nicht setzbar")
        }
    })();
    if let Err(e) = result {
        eprintln!("Warnung: {e:#}");
    }
}

const FIREWALL_RULE: &str = "CTXRemote Direktverbindung";

/// Opens the direct-connection port for the service only. Failures are
/// reported but not fatal: sessions then simply stay on the relay.
fn allow_direct_connections(enabled: bool, port: u16) {
    remove_firewall_rule();
    if !enabled {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let status = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "add", "rule"])
        .arg(format!("name={FIREWALL_RULE}"))
        .args(["dir=in", "action=allow", "protocol=TCP", "profile=any", "enable=yes"])
        .arg(format!("localport={port}"))
        .arg(format!("program={}", exe.display()))
        .stdout(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => println!("Firewall: Port {port} für Direktverbindungen geöffnet."),
        _ => eprintln!("Firewall-Regel konnte nicht angelegt werden; Sitzungen laufen dann über den Server."),
    }
}

fn remove_firewall_rule() {
    let _ = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "delete", "rule"])
        .arg(format!("name={FIREWALL_RULE}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Enables Ctrl+Alt+Del from a service: SoftwareSASGeneration 0/missing -> 1, 2 -> 3 (1 and 3 stay).
fn enable_sas_policy() -> Result<()> {
    let key_path = wide(std::ffi::OsStr::new(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System",
    ));
    let name = wide(std::ffi::OsStr::new("SoftwareSASGeneration"));
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key_path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
        .context("Richtlinie SoftwareSASGeneration: Schlüssel nicht öffnbar")?;

        let mut data = [0u8; 4];
        let mut len = data.len() as u32;
        let mut kind = REG_VALUE_TYPE::default();
        let query = RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut len),
        );
        let valid = query.is_ok() && kind == REG_DWORD;
        let current = if valid { u32::from_le_bytes(data) } else { 0 };
        let wanted = match current {
            2 => 3,
            1 | 3 => current,
            _ => 1,
        };
        let result = if !valid || wanted != current {
            RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&wanted.to_le_bytes()))
                .ok()
                .context("Richtlinie SoftwareSASGeneration nicht setzbar")
        } else {
            Ok(())
        };
        let _ = RegCloseKey(key);
        result
    }
}

/// Starts the host and, next to it, the app's link (UI pipe).
fn start_host(config: Config, screen: Arc<dyn ScreenSource>) -> Host {
    let config = Arc::new(RwLock::new(config));
    let host = Host::start_with(config.clone(), screen);
    let link_host = host.clone();
    let link_config = config.clone();
    tokio::spawn(async move {
        if let Err(e) = ui_link::serve(link_host, link_config).await {
            tracing::error!("Verbindung zur App beendet: {e:#}");
        }
    });
    tokio::spawn(auto_update(host.clone(), config));
    host
}

// ------------------------------------------------------------ updates

/// First check a little after start, so a boot is not slowed down.
const UPDATE_FIRST_CHECK: Duration = Duration::from_secs(120);
/// Fallback when the server stays up; a release restarts it (see below).
const UPDATE_INTERVAL: Duration = Duration::from_secs(3600);
/// While someone is connected, check again this often whether they are done.
const UPDATE_IDLE_POLL: Duration = Duration::from_secs(300);
/// Spreads the devices' checks after a server restart over this long.
const UPDATE_SPREAD_SECS: u64 = 180;

/// Installs signed releases from the server on its own, but only while
/// nobody controls this device: the installer restarts the service.
///
/// A release restarts the server, so every device goes offline for a moment;
/// coming back online is the cue to look for an update right away instead
/// of waiting for the next hourly check.
async fn auto_update(host: Host, config: Arc<RwLock<Config>>) {
    if !ctxremote_core::update::enabled() {
        tracing::info!("Automatische Updates sind in diesem Build nicht eingerichtet");
        return;
    }
    let mut presence = host.presence();
    tokio::time::sleep(UPDATE_FIRST_CHECK).await;
    presence.borrow_and_update();
    loop {
        if let Err(e) = update_once(&host, &config).await {
            tracing::warn!("Update fehlgeschlagen: {e:#}");
        }
        let back_online = async {
            let mut was_online = matches!(*presence.borrow_and_update(), Presence::Online { .. });
            loop {
                if presence.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
                let online = matches!(*presence.borrow_and_update(), Presence::Online { .. });
                if online && !was_online {
                    return;
                }
                was_online = online;
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(UPDATE_INTERVAL) => {}
            _ = back_online => {
                // Not all devices at the same moment: the server just started.
                let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
                tokio::time::sleep(Duration::from_secs(30 + u64::from(nanos) % UPDATE_SPREAD_SECS)).await;
            }
        }
    }
}

async fn update_once(host: &Host, config: &RwLock<Config>) -> Result<()> {
    use ctxremote_core::update;
    let server = config.read().unwrap().server_addr();
    let Some(info) = update::check(&server).await? else { return Ok(()) };
    tracing::info!(version = %info.version, "Update verfügbar");
    while !host.sessions().is_empty() {
        tokio::time::sleep(UPDATE_IDLE_POLL).await;
    }
    // Below the service's data folder, which only SYSTEM and administrators may
    // write: nobody can swap the installer between check and start.
    let dir = data_dir().join("updates");
    let _ = std::fs::remove_dir_all(&dir);
    let installer = update::download(&server, &info, &dir).await?;
    // Someone may have connected during the download; then wait for the next round.
    if !host.sessions().is_empty() {
        return Ok(());
    }
    tracing::info!(version = %info.version, "Update wird installiert, der Dienst startet gleich neu");
    // The installer closes the app; the next service start brings it back.
    let _ = std::fs::write(data_dir().join(RELAUNCH_MARKER), b"");
    update::launch(&installer, update::InstallMode::Silent)
}

/// Written before a silent update; its presence at start means "start the app again".
const RELAUNCH_MARKER: &str = "relaunch-app";

/// After a silent update: starts the app (in the tray) for the signed-in user,
/// since the installer had to close it.
fn relaunch_app_after_update() {
    let marker = data_dir().join(RELAUNCH_MARKER);
    if std::fs::remove_file(&marker).is_err() {
        return;
    }
    let Some(app) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("ctxremote.exe"))) else {
        return;
    };
    std::thread::spawn(move || {
        // Give the desktop a moment if the update ran right after sign-in.
        std::thread::sleep(Duration::from_secs(3));
        match ctxremote_core::update::start_for_console_user(&app, "--tray") {
            Ok(()) => tracing::info!("App nach dem Update wieder gestartet"),
            Err(e) => tracing::info!("App nach dem Update nicht gestartet: {e:#}"),
        }
    });
}

// ------------------------------------------------------------ configure

#[derive(serde::Deserialize)]
struct SettingsRequest {
    #[serde(default)]
    server: String,
    permanent_password: Option<String>,
    /// Set instead of the server settings when only the direct connection changes.
    #[serde(default)]
    direct: Option<DirectSettings>,
    /// Set instead when only the access for the account's devices changes.
    #[serde(default)]
    access: Option<AccessChange>,
    /// Set instead when only the rights of new sessions change.
    #[serde(default)]
    rights: Option<RightsChange>,
    /// Set instead when only the authenticator secret changes.
    #[serde(default)]
    code: Option<CodeChange>,
}

#[derive(serde::Deserialize)]
struct CodeChange {
    secret: Option<String>,
}

#[derive(serde::Deserialize)]
struct RightsChange {
    attended: ctxremote_core::proto::session::Permissions,
    unattended: ctxremote_core::proto::session::Permissions,
}

#[derive(serde::Deserialize)]
struct AccessChange {
    grant: Option<ctxremote_core::account::AccessGrant>,
}

/// The app's elevated helper: hands the settings in `file` to the running
/// service and writes the answer back into the file.
///
/// The file lives in the user's temp folder, so it is only trusted if it still
/// matches the digest the app passed on the command line. Otherwise nothing is
/// applied and nothing is written, so the path cannot be used to overwrite files.
fn configure(file: &Path, digest: &str) -> Result<()> {
    let request = std::fs::read(file).context("Anfrage nicht lesbar")?;
    if hex::encode(Sha256::digest(&request)) != digest.to_ascii_lowercase() {
        bail!("Die Anfrage wurde verändert und nicht übernommen");
    }
    let answer = apply_settings(&request);
    let reply = match &answer {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "error": e }),
    };
    std::fs::write(file, reply.to_string()).context("Antwort nicht schreibbar")?;
    answer.map_err(anyhow::Error::msg)
}

fn apply_settings(request: &[u8]) -> Result<(), String> {
    let request: SettingsRequest =
        serde_json::from_slice(request).map_err(|e| format!("Anfrage ungültig: {e}"))?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let (answers, mut answer) = tokio::sync::mpsc::unbounded_channel();
        let link = ServiceLink::connect(move |event| {
            if let Some(UiEvent::Configured(result)) = event {
                let _ = answers.send(result);
            }
        })
        .await
        .map_err(|e| format!("{e:#}"))?;
        let direct = request.direct.clone();
        match (&direct, request.access, request.rights, request.code) {
            (Some(settings), ..) => link.send(UiRequest::ConfigureDirect(settings.clone())),
            (None, Some(access), ..) => link.send(UiRequest::ConfigureAccountAccess(access.grant)),
            (None, None, Some(rights), _) => {
                link.send(UiRequest::ConfigureRights { attended: rights.attended, unattended: rights.unattended })
            }
            (None, None, None, Some(code)) => link.send(UiRequest::ConfigureCode(code.secret)),
            (None, None, None, None) => link.send(UiRequest::Configure {
                server: request.server,
                permanent_password: request.permanent_password,
            }),
        }
        let result = match tokio::time::timeout(Duration::from_secs(10), answer.recv()).await {
            Ok(Some(result)) => result,
            _ => Err("Der Dienst hat nicht geantwortet".into()),
        };
        // This helper runs elevated, so it may adjust the firewall; the service cannot.
        if let (Ok(()), Some(settings)) = (&result, &direct) {
            allow_direct_connections(settings.enabled, settings.port);
        }
        result
    })
}
