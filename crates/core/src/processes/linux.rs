//! Services on Linux: systemd's units, through `systemctl`. The agent runs
//! as the signed-in user, so starting and stopping system services needs
//! a policy that allows it; systemd's refusal is passed on as it is.

use std::collections::HashMap;
use std::process::Command;

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::{ServiceAction, ServiceInfo, ServiceState, StartMode};

fn systemctl(args: &[&str]) -> Result<std::process::Output> {
    Command::new("systemctl").args(args).env("SYSTEMD_COLORS", "0").output().context("systemctl nicht verfügbar")
}

/// `None` without systemd.
pub fn list() -> Result<Option<Vec<ServiceInfo>>> {
    let Ok(units) = systemctl(&["list-units", "--type=service", "--all", "--plain", "--no-legend", "--no-pager"]) else {
        return Ok(None);
    };
    if !units.status.success() {
        return Ok(None);
    }
    let files = systemctl(&["list-unit-files", "--type=service", "--plain", "--no-legend", "--no-pager"])?;
    let modes: HashMap<String, StartMode> = String::from_utf8_lossy(&files.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.to_string(), start_mode(fields.next()?)))
        })
        .collect();

    let mut services: Vec<ServiceInfo> = String::from_utf8_lossy(&units.stdout)
        .lines()
        .filter_map(|line| {
            // Failed units may carry a marker in front.
            let line = line.trim_start_matches(['●', '*', ' ']);
            let mut fields = line.split_whitespace();
            let name = fields.next()?.to_string();
            let _load = fields.next()?;
            let active = fields.next()?;
            let _sub = fields.next()?;
            let display = fields.collect::<Vec<_>>().join(" ");
            Some(ServiceInfo {
                start: modes.get(&name).copied().unwrap_or(StartMode::Other),
                state: state(active),
                display,
                name,
                pid: None,
            })
        })
        .collect();
    // Installed but never loaded since boot: stopped. Templates (`x@.service`) cannot run as they are.
    for (name, start) in &modes {
        if !name.ends_with("@.service") && !services.iter().any(|s| &s.name == name) {
            services.push(ServiceInfo {
                name: name.clone(),
                display: String::new(),
                state: ServiceState::Stopped,
                start: *start,
                pid: None,
            });
        }
    }
    services.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Some(services))
}

fn state(active: &str) -> ServiceState {
    match active {
        "active" | "reloading" => ServiceState::Running,
        "inactive" | "failed" => ServiceState::Stopped,
        "activating" => ServiceState::Starting,
        "deactivating" => ServiceState::Stopping,
        _ => ServiceState::Other,
    }
}

fn start_mode(state: &str) -> StartMode {
    match state {
        "enabled" | "enabled-runtime" => StartMode::Automatic,
        "disabled" => StartMode::Manual,
        "masked" | "masked-runtime" => StartMode::Disabled,
        _ => StartMode::Other,
    }
}

pub fn act(name: &str, action: ServiceAction) -> Result<()> {
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || "@._:-\\".contains(c)) {
        bail!("Unbekannter Dienst");
    }
    let verb = match action {
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
    };
    // Never asks a password at the host: that would wait for nobody.
    let out = systemctl(&["--no-ask-password", verb, "--", name])?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if text.contains("authentication required") || text.contains("Access denied") {
            bail!("Keine Berechtigung (Remocto läuft hier als angemeldeter Benutzer): {text}");
        }
        bail!("{}", if text.is_empty() { "systemctl ist fehlgeschlagen".to_string() } else { text });
    }
    Ok(())
}
