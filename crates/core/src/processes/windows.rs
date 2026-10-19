//! Services on Windows, through the service control manager. Under the
//! Remocto service the agent runs as SYSTEM and may control them all.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use ctxremote_proto::session::{ServiceAction, ServiceInfo, ServiceState, StartMode};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DEPENDENT_SERVICES_RUNNING, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA,
    ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DISABLED, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceConfigW,
    QueryServiceStatus, StartServiceW, ENUM_SERVICE_STATUS_PROCESSW, QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_AUTO_START, SERVICE_BOOT_START,
    SERVICE_CONTROL_STOP, SERVICE_CONTINUE_PENDING, SERVICE_DEMAND_START, SERVICE_DISABLED, SERVICE_PAUSED,
    SERVICE_PAUSE_PENDING, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START,
    SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP,
    SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_SYSTEM_START, SERVICE_WIN32,
};

/// Closes the handle when dropped.
struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> Result<Handle> {
    let handle = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access) }
        .map_err(|e| anyhow::anyhow!("Dienstverwaltung nicht erreichbar: {}", e.message()))?;
    Ok(Handle(handle))
}

pub fn list() -> Result<Option<Vec<ServiceInfo>>> {
    let scm = manager(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut services = Vec::new();
    let mut resume = 0u32;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let (mut needed, mut count) = (0u32, 0u32);
        let result = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(&mut buffer),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        let more = match result {
            Ok(()) => false,
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => true,
            Err(e) => bail!("Dienste nicht lesbar: {}", e.message()),
        };
        // SAFETY: the call filled `count` entries at the start of the buffer;
        // their strings point into the same buffer.
        let entries =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize) };
        for entry in entries {
            let name = unsafe { entry.lpServiceName.to_string() }.unwrap_or_default();
            let display = unsafe { entry.lpDisplayName.to_string() }.unwrap_or_default();
            let status = entry.ServiceStatusProcess;
            let pid = (status.dwProcessId != 0).then_some(status.dwProcessId);
            services.push(ServiceInfo {
                start: start_mode(&scm, &name),
                state: state(status.dwCurrentState),
                name,
                display,
                pid,
            });
        }
        if !more {
            break;
        }
        if needed as usize > buffer.len() {
            buffer.resize(needed as usize, 0);
        }
    }
    services.sort_by_key(|s| s.display.to_lowercase());
    Ok(Some(services))
}

fn state(state: SERVICE_STATUS_CURRENT_STATE) -> ServiceState {
    match state {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED => ServiceState::Stopped,
        SERVICE_START_PENDING | SERVICE_CONTINUE_PENDING => ServiceState::Starting,
        SERVICE_STOP_PENDING | SERVICE_PAUSE_PENDING => ServiceState::Stopping,
        SERVICE_PAUSED => ServiceState::Paused,
        _ => ServiceState::Other,
    }
}

fn start_mode(scm: &Handle, name: &str) -> StartMode {
    let Ok(service) = (unsafe { OpenServiceW(scm.0, &HSTRING::from(name), SERVICE_QUERY_CONFIG) }) else {
        return StartMode::Other;
    };
    let service = Handle(service);
    let mut needed = 0u32;
    // The first call says how much room the configuration needs.
    if let Err(e) = unsafe { QueryServiceConfigW(service.0, None, 0, &mut needed) } {
        if e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return StartMode::Other;
        }
    }
    // u64 keeps the structure aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    let config = buffer.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
    if unsafe { QueryServiceConfigW(service.0, Some(config), needed, &mut needed) }.is_err() {
        return StartMode::Other;
    }
    match unsafe { (*config).dwStartType } {
        SERVICE_AUTO_START | SERVICE_BOOT_START | SERVICE_SYSTEM_START => StartMode::Automatic,
        SERVICE_DEMAND_START => StartMode::Manual,
        SERVICE_DISABLED => StartMode::Disabled,
        _ => StartMode::Other,
    }
}

/// How long a service may take to start or stop.
const WAIT: Duration = Duration::from_secs(30);

pub fn act(name: &str, action: ServiceAction) -> Result<()> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let service = unsafe { OpenServiceW(scm.0, &HSTRING::from(name), SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS) }
        .map_err(|e| explain(e, name))?;
    let service = Handle(service);
    if matches!(action, ServiceAction::Stop | ServiceAction::Restart) {
        let mut status = SERVICE_STATUS::default();
        match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_SERVICE_NOT_ACTIVE.to_hresult() => {}
            Err(e) => return Err(explain(e, name)),
        }
        wait_for(&service, SERVICE_STOPPED, name)?;
    }
    if matches!(action, ServiceAction::Start | ServiceAction::Restart) {
        match unsafe { StartServiceW(service.0, None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => {}
            Err(e) => return Err(explain(e, name)),
        }
        wait_for(&service, SERVICE_RUNNING, name)?;
    }
    Ok(())
}

fn wait_for(service: &Handle, wanted: SERVICE_STATUS_CURRENT_STATE, name: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        let mut status = SERVICE_STATUS::default();
        unsafe { QueryServiceStatus(service.0, &mut status) }.map_err(|e| explain(e, name))?;
        if status.dwCurrentState == wanted {
            return Ok(());
        }
        // Gave up starting: it stopped again.
        if wanted == SERVICE_RUNNING && status.dwCurrentState == SERVICE_STOPPED && start.elapsed() > Duration::from_secs(1) {
            bail!("{name} hat sich gleich wieder beendet (Fehlercode {})", status.dwWin32ExitCode);
        }
        if start.elapsed() > WAIT {
            bail!("{name} antwortet nicht (nach {} Sekunden)", WAIT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn explain(e: windows::core::Error, name: &str) -> anyhow::Error {
    let code = e.code();
    if code == ERROR_ACCESS_DENIED.to_hresult() {
        anyhow::anyhow!("Keine Berechtigung für den Dienst {name} (nur mit installiertem Remocto-Dienst)")
    } else if code == ERROR_DEPENDENT_SERVICES_RUNNING.to_hresult() {
        anyhow::anyhow!("Andere laufende Dienste hängen von {name} ab")
    } else if code == ERROR_SERVICE_DISABLED.to_hresult() {
        anyhow::anyhow!("{name} ist deaktiviert")
    } else if code == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() {
        anyhow::anyhow!("Den Dienst {name} gibt es nicht")
    } else {
        anyhow::anyhow!("{name}: {}", e.message())
    }
}
