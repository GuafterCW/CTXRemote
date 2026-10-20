//! The host's processes and services for the viewer (see [`ProcessMsg`]):
//! listed, ended, started and stopped without the picture.
//!
//! Runs in the agent, with its rights: SYSTEM under the Windows service,
//! which is why [`Permissions::PROCESSES`] is off unless switched on. Work
//! that can take a while (listing, waiting for a service) happens on
//! blocking threads; answers go out through the agent's outbox.
//!
//! [`Permissions::PROCESSES`]: ctxremote_proto::session::Permissions::PROCESSES

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use ctxremote_proto::session::{HostMsg, ProcessInfo, ProcessListing, ProcessMsg, ServiceAction};
use sysinfo::{MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind, Users};
use tokio::sync::mpsc;
use tracing::{info, warn};

#[cfg(windows)]
#[path = "windows.rs"]
mod services;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod services;
#[cfg(not(any(windows, target_os = "linux")))]
mod services {
    use ctxremote_proto::session::{ServiceAction, ServiceInfo};

    pub fn list() -> anyhow::Result<Option<Vec<ServiceInfo>>> {
        Ok(None)
    }

    pub fn act(_name: &str, _action: ServiceAction) -> anyhow::Result<()> {
        anyhow::bail!("Dienste lassen sich auf diesem Gerät nicht steuern")
    }
}

/// Ending one of these takes the computer down with it.
#[cfg(windows)]
const VITAL: &[&str] =
    &["system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "memory compression"];
#[cfg(not(windows))]
const VITAL: &[&str] = &["init", "systemd", "launchd", "kernel_task"];

/// The session's view of the processes; keeps the previous reading so CPU
/// use can be measured between two lists.
pub struct Processes {
    outbox: mpsc::Sender<HostMsg>,
    system: Arc<Mutex<System>>,
}

impl Processes {
    pub fn new(outbox: mpsc::Sender<HostMsg>) -> Self {
        Self { outbox, system: Arc::new(Mutex::new(System::new())) }
    }

    pub fn handle(&self, msg: ProcessMsg) {
        let outbox = self.outbox.clone();
        let system = self.system.clone();
        let reply = move || match msg {
            ProcessMsg::List => Some(ProcessMsg::Listing(list(&system).map_err(|e| format!("{e:#}")))),
            ProcessMsg::Kill { pid, name } => {
                let result = kill(&system, pid, &name);
                info!(pid, name, ok = result.is_ok(), "Prozess beenden");
                Some(ProcessMsg::Done { what: format!("Prozess {name} beendet"), result: result.map_err(|e| format!("{e:#}")) })
            }
            ProcessMsg::Service { name, action } => {
                let result = service(&name, action);
                info!(name, ?action, ok = result.is_ok(), "Dienst");
                let what = match action {
                    ServiceAction::Start => format!("Dienst {name} gestartet"),
                    ServiceAction::Stop => format!("Dienst {name} angehalten"),
                    ServiceAction::Restart => format!("Dienst {name} neu gestartet"),
                };
                Some(ProcessMsg::Done { what, result: result.map_err(|e| format!("{e:#}")) })
            }
            // Only the host sends these.
            ProcessMsg::Listing(_) | ProcessMsg::Done { .. } => None,
        };
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(reply).await {
                Ok(Some(reply)) => {
                    let _ = outbox.send(HostMsg::Processes(reply)).await;
                }
                Ok(None) => {}
                Err(e) => warn!("Prozesse: {e}"),
            }
        });
    }
}

fn refresh(system: &mut System) {
    let kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_user(UpdateKind::OnlyIfNotSet);
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
}

fn list(system: &Mutex<System>) -> Result<ProcessListing> {
    let mut system = system.lock().unwrap();
    let first = system.processes().is_empty();
    refresh(&mut system);
    if first {
        // CPU use is the difference between two readings.
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)));
        refresh(&mut system);
    }
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    let users = Users::new_with_refreshed_list();
    let processes = system
        .processes()
        .values()
        // Threads show up as processes on Linux.
        .filter(|p| p.thread_kind().is_none())
        .map(|p| ProcessInfo {
            pid: p.pid().as_u32(),
            parent: p.parent().map(|parent| parent.as_u32()),
            name: p.name().to_string_lossy().into_owned(),
            user: p
                .user_id()
                .and_then(|id| users.get_user_by_id(id))
                .map(|u| u.name().to_string())
                .unwrap_or_default(),
            cpu: p.cpu_usage(),
            memory: p.memory(),
            exe: p.exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        })
        .collect();
    let services = services::list().unwrap_or_else(|e| {
        warn!("Dienste nicht lesbar: {e:#}");
        None
    });
    Ok(ProcessListing {
        processes,
        services,
        cpus: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
        memory_total: system.total_memory(),
    })
}

fn kill(system: &Mutex<System>, pid: u32, name: &str) -> Result<()> {
    let mut system = system.lock().unwrap();
    let id = Pid::from_u32(pid);
    // This agent and what started it (the service or the app) carry the session.
    let own = Pid::from_u32(std::process::id());
    system.refresh_processes(ProcessesToUpdate::Some(&[id, own]), true);
    let starter = system.process(own).and_then(|p| p.parent());
    let Some(process) = system.process(id) else { bail!("Der Prozess läuft nicht mehr") };
    if process.name().to_string_lossy() != name {
        bail!("Unter dieser Nummer läuft inzwischen ein anderer Prozess");
    }
    if id == own || Some(id) == starter {
        bail!("Remocto selbst lässt sich hier nicht beenden");
    }
    if pid <= 4 || VITAL.contains(&name.to_lowercase().as_str()) {
        bail!("Ohne {name} läuft das System nicht weiter, darum wird der Prozess nicht beendet");
    }
    // Asked to end first where the system can ask; forced if it does not.
    let asked = cfg!(unix) && process.kill_with(Signal::Term).unwrap_or(false);
    if asked && gone(&mut system, id, Duration::from_secs(3)) {
        return Ok(());
    }
    let Some(process) = system.process(id) else { return Ok(()) };
    if !process.kill() {
        bail!("Keine Berechtigung, {name} zu beenden");
    }
    if !gone(&mut system, id, Duration::from_secs(3)) {
        bail!("{name} läuft weiter");
    }
    Ok(())
}

fn gone(system: &mut System, id: Pid, within: Duration) -> bool {
    let start = Instant::now();
    loop {
        system.refresh_processes(ProcessesToUpdate::Some(&[id]), true);
        if system.process(id).is_none_or(|p| p.status() == sysinfo::ProcessStatus::Zombie) {
            return true;
        }
        if start.elapsed() > within {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn service(name: &str, action: ServiceAction) -> Result<()> {
    // Remocto's own service carries this session.
    if name.eq_ignore_ascii_case("Remocto") || name.eq_ignore_ascii_case("remocto.service") {
        bail!("Remocto selbst lässt sich hier nicht anhalten");
    }
    if name.is_empty() || name.starts_with('-') {
        bail!("Unbekannter Dienst");
    }
    services::act(name, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_this_process() {
        let system = Mutex::new(System::new());
        let listing = list(&system).unwrap();
        let own = listing.processes.iter().find(|p| p.pid == std::process::id()).expect("eigener Prozess fehlt");
        assert!(own.memory > 0);
    }
}
