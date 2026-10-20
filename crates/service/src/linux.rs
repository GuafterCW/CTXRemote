//! The Remocto host without a desktop, for Linux servers: runs under systemd
//! as an ordinary user and offers terminal, files and processes (the agent
//! finds no screen, see `ctxremote_core::agent::run`).
//!
//! The config is the user's own (`~/.config/remocto/config.json`), so the
//! ID stays the same when the app is used on the same account later.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use ctxremote_core::config::Config;
use ctxremote_core::host::{Host, Presence};

const UNIT_PATH: &str = "/etc/systemd/system/remocto.service";
const INSTALLED: &str = "/usr/local/bin/remocto-service";

const USAGE: &str = "Remocto für Linux-Server (ohne Bildschirm: Terminal, Dateien, Prozesse)

  remocto-service --console            Host im Vordergrund starten (Ende mit Strg+C)
  remocto-service --set-password       festes Passwort setzen (von der Eingabe, mindestens 8 Zeichen)
  remocto-service --set-server ADRESSE eigenen Server verwenden
  remocto-service --id                 ID dieses Geräts anzeigen
  remocto-service --install [--user NAME] [--server ADRESSE]
                                       nach /usr/local/bin kopieren, als systemd-Dienst
                                       einrichten und starten (als root)
  remocto-service --uninstall          systemd-Dienst entfernen (als root)";

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["--console"] => console(),
        ["--set-password"] => set_password(),
        ["--set-server", server] => set_server(server),
        ["--id"] => show_id(),
        ["--install", rest @ ..] => install(rest),
        ["--uninstall"] => uninstall(),
        _ => {
            eprintln!("{USAGE}");
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

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc_geteuid() == 0 }
}

extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

/// The terminal would be a root shell for everyone with the password.
fn refuse_root() -> Result<()> {
    if is_root() {
        bail!("Nicht als root starten: Das Terminal liefe sonst mit allen Rechten. Mit --install als eigener Benutzer einrichten.");
    }
    Ok(())
}

fn console() -> Result<()> {
    refuse_root()?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // systemd's journal adds its own time stamps.
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stderr().is_terminal()).without_time().init();
    let config = Config::load()?;
    if config.permanent_password.as_deref().is_none_or(str::is_empty) {
        tracing::warn!("Kein festes Passwort gesetzt: remocto-service --set-password");
    }
    let config = Arc::new(RwLock::new(config));
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let host = Host::start(config);
        let mut presence = host.presence();
        let report = async {
            loop {
                match &*presence.borrow_and_update() {
                    Presence::Online { id } => tracing::info!(id, "Erreichbar"),
                    Presence::Offline { reason } => tracing::warn!(reason, "Nicht erreichbar"),
                    Presence::Connecting => {}
                }
                if presence.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        };
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = report => {}
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        anyhow::Ok(())
    })?;
    tracing::info!("Host beendet");
    Ok(())
}

fn set_password() -> Result<()> {
    refuse_root()?;
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprint!("Festes Passwort (wird angezeigt, mindestens 8 Zeichen): ");
        std::io::stderr().flush()?;
    }
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']);
    if password.is_empty() {
        bail!("Kein Passwort eingegeben");
    }
    let mut config = Config::load()?;
    let server = config.server.clone();
    config.apply_settings(&server, Some(password))?;
    config.save()?;
    println!("Festes Passwort gesetzt. Ein laufender Host übernimmt es beim nächsten Start (systemctl restart remocto).");
    Ok(())
}

fn set_server(server: &str) -> Result<()> {
    refuse_root()?;
    let mut config = Config::load()?;
    config.apply_settings(server, None)?;
    config.save()?;
    println!("Server gesetzt: {}", server.trim());
    Ok(())
}

fn show_id() -> Result<()> {
    let config = Config::load()?;
    match config.device_id {
        Some(id) => println!("{id}"),
        None => println!("Noch keine ID: Den Host einmal starten, er bekommt sie vom Server."),
    }
    Ok(())
}

/// Writes the systemd unit for `--user` (default: who called sudo) and starts it.
fn install(args: &[&str]) -> Result<()> {
    let (mut user, mut server) = (std::env::var("SUDO_USER").ok(), None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--user" => user = Some(rest.next().context("--user braucht einen Namen")?.to_string()),
            "--server" => server = Some(rest.next().context("--server braucht eine Adresse")?.to_string()),
            other => bail!("Unbekannte Angabe {other}\n\n{USAGE}"),
        }
    }
    if !is_root() {
        bail!("--install braucht root (sudo)");
    }
    let user = user.filter(|u| !u.is_empty()).context("Benutzer mit --user NAME angeben")?;
    if user == "root" || !user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        bail!("Ungeeigneter Benutzer {user:?}: Der Dienst läuft als eigener Benutzer, nicht als root");
    }
    // Wherever it was downloaded to, the service runs a copy only root can change.
    let current = std::env::current_exe().context("Programmpfad unbekannt")?;
    if current != Path::new(INSTALLED) {
        let tmp = format!("{INSTALLED}.new");
        std::fs::copy(&current, &tmp).with_context(|| format!("{INSTALLED} nicht schreibbar"))?;
        std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        std::fs::rename(&tmp, INSTALLED)?;
    }
    let exe = INSTALLED;
    // The server is a per-user setting; set it as that user, before the start.
    if let Some(server) = server {
        let status = std::process::Command::new("runuser")
            .args(["-u", &user, "--", exe, "--set-server", &server])
            .status()
            .context("runuser nicht gefunden")?;
        if !status.success() {
            bail!("Serveradresse nicht gesetzt");
        }
    }
    std::fs::write(UNIT_PATH, unit(&user, exe)).with_context(|| format!("{UNIT_PATH} nicht schreibbar"))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", "remocto.service"])?;
    println!("Remocto läuft als Dienst für {user}.");
    println!("Passwort setzen: sudo -H -u {user} remocto-service --set-password && sudo systemctl restart remocto");
    println!("ID anzeigen:     sudo -H -u {user} remocto-service --id   (Protokoll: journalctl -u remocto)");
    Ok(())
}

fn unit(user: &str, exe: &str) -> String {
    format!(
        "[Unit]
Description=Remocto (Fernzugriff ohne Bildschirm)
Wants=network-online.target
After=network-online.target

[Service]
User={user}
ExecStart={exe} --console
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
"
    )
}

fn uninstall() -> Result<()> {
    if !is_root() {
        bail!("--uninstall braucht root (sudo)");
    }
    if Path::new(UNIT_PATH).exists() {
        let _ = systemctl(&["disable", "--now", "remocto.service"]);
        std::fs::remove_file(UNIT_PATH)?;
        systemctl(&["daemon-reload"])?;
    }
    let _ = std::fs::remove_file(INSTALLED);
    println!("Dienst entfernt. Die Einstellungen (ID, Passwort) bleiben im Konfigurationsordner des Benutzers.");
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("systemctl").args(args).status().context("systemctl nicht gefunden")?;
    if !status.success() {
        bail!("systemctl {} fehlgeschlagen", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_as_the_user() {
        let unit = unit("ops", "/opt/remocto/remocto-service");
        assert!(unit.contains("User=ops\n"));
        assert!(unit.contains("ExecStart=/opt/remocto/remocto-service --console\n"));
    }
}
