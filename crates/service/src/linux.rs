//! The Remocto host without a desktop, for Linux servers: runs under systemd
//! as an ordinary user and offers terminal, files and processes (the agent
//! finds no screen, see `ctxremote_core::agent::run`).
//!
//! Updates: a root timer (`remocto-update.timer`) runs `--update`, which
//! puts a signed newer binary in place; the running host notices and
//! restarts once nobody is connected (systemd starts the new one).
//!
//! The config is the user's own (`~/.config/remocto/config.json`), so the
//! ID stays the same when the app is used on the same account later.

use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_core::config::Config;
use ctxremote_core::host::{Host, Presence};
use ctxremote_core::update;

const UNIT_PATH: &str = "/etc/systemd/system/remocto.service";
const UPDATE_UNIT_PATH: &str = "/etc/systemd/system/remocto-update.service";
const TIMER_PATH: &str = "/etc/systemd/system/remocto-update.timer";
const INSTALLED: &str = "/usr/local/bin/remocto-service";
/// Where `--update` downloads to; root's alone.
const CACHE: &str = "/var/cache/remocto";

const USAGE: &str = "Remocto für Linux-Server (ohne Bildschirm: Terminal, Dateien, Prozesse)

  remocto-service --console            Host im Vordergrund starten (Ende mit Strg+C)
  remocto-service --set-password       festes Passwort setzen (von der Eingabe, mindestens 8 Zeichen)
  remocto-service --set-server ADRESSE eigenen Server verwenden
  remocto-service --id                 ID dieses Geräts anzeigen
  remocto-service --install [--user NAME] [--server ADRESSE]
                                       nach /usr/local/bin kopieren, als systemd-Dienst
                                       einrichten und starten (als root)
  remocto-service --update             neuere Version vom Server holen (als root; läuft
                                       sonst alle 6 Stunden von selbst)
  remocto-service --uninstall          systemd-Dienst entfernen (als root)";

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["--console"] => console(),
        ["--set-password"] => set_password(),
        ["--set-server", server] => set_server(server),
        ["--id"] => show_id(),
        // For `--update` (root), which needs the service user's server.
        ["--server"] => show_server(),
        ["--update"] => update(),
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
    // Replaced by `--update` while running: restart into the new version.
    let exe = std::env::current_exe().ok().filter(|exe| exe == Path::new(INSTALLED));
    let started = exe.as_deref().and_then(binary_id);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let host = Host::start(config);
        let replaced = {
            let host = host.clone();
            async move {
                let Some(started) = started else { return std::future::pending::<()>().await };
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tick.tick().await;
                    if binary_id(Path::new(INSTALLED)).is_some_and(|now| now != started) && host.sessions().is_empty() {
                        return;
                    }
                }
            }
        };
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
            _ = replaced => tracing::info!("Neue Version eingesetzt, Neustart"),
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

/// Which file a path names now: changes when `--update` replaces it.
fn binary_id(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

fn show_server() -> Result<()> {
    println!("{}", Config::load()?.server);
    Ok(())
}

/// `--update` as root: fetches a newer signed release of this binary and puts
/// it in place. The running host restarts itself when idle.
fn update() -> Result<()> {
    if !is_root() {
        bail!("--update braucht root (sudo)");
    }
    if !update::has_key() {
        println!("Dieser Build aktualisiert sich nicht selbst.");
        return Ok(());
    }
    let user = service_user()?;
    let output = std::process::Command::new("runuser")
        .args(["-u", &user, "--", INSTALLED, "--server"])
        .output()
        .context("runuser nicht gefunden")?;
    let server = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || server.is_empty() {
        bail!("Serveradresse von {user} nicht lesbar");
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(CACHE)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        // Verified against the key built in, and only if newer than this one.
        let Some(info) = update::check_platform(&server, update::LINUX_SERVER).await? else {
            println!("Remocto {} ist aktuell.", update::VERSION);
            return Ok(());
        };
        let file = update::download(&server, &info, Path::new(CACHE)).await?;
        let placed = put_in_place(&file);
        let _ = std::fs::remove_file(&file);
        placed?;
        println!("Remocto {} eingesetzt. Der Dienst startet neu, sobald niemand verbunden ist.", info.version);
        anyhow::Ok(())
    })
}

/// The `User=` of the installed unit.
fn service_user() -> Result<String> {
    let unit = std::fs::read_to_string(UNIT_PATH).context("Nicht als Dienst eingerichtet (--install)")?;
    unit.lines()
        .find_map(|line| line.strip_prefix("User="))
        .map(|user| user.trim().to_string())
        .filter(|user| !user.is_empty() && user != "root")
        .context("Kein Benutzer in remocto.service")
}

/// Copies `file` over the installed binary in one step; a running host keeps
/// its old file until it restarts.
fn put_in_place(file: &Path) -> Result<()> {
    let tmp = format!("{INSTALLED}.new");
    std::fs::copy(file, &tmp).with_context(|| format!("{INSTALLED} nicht schreibbar"))?;
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    std::fs::rename(&tmp, INSTALLED)?;
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
        put_in_place(&current)?;
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
    std::fs::write(UPDATE_UNIT_PATH, update_unit(exe))?;
    std::fs::write(TIMER_PATH, UPDATE_TIMER)?;
    systemctl(&["daemon-reload"])?;
    // A second --install (e.g. another user) takes effect now.
    systemctl(&["enable", "remocto.service"])?;
    systemctl(&["restart", "remocto.service"])?;
    systemctl(&["enable", "--now", "remocto-update.timer"])?;
    println!("Remocto läuft als Dienst für {user} und sieht alle 6 Stunden nach Updates.");
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

fn update_unit(exe: &str) -> String {
    format!(
        "[Unit]
Description=Remocto: neuere Version holen
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart={exe} --update
"
    )
}

const UPDATE_TIMER: &str = "[Unit]
Description=Remocto: alle 6 Stunden nach Updates sehen

[Timer]
OnBootSec=10min
OnUnitActiveSec=6h
RandomizedDelaySec=30min

[Install]
WantedBy=timers.target
";

fn uninstall() -> Result<()> {
    if !is_root() {
        bail!("--uninstall braucht root (sudo)");
    }
    let units = [(TIMER_PATH, "remocto-update.timer"), (UPDATE_UNIT_PATH, "remocto-update.service"), (UNIT_PATH, "remocto.service")];
    let mut changed = false;
    for (path, name) in units {
        if Path::new(path).exists() {
            let _ = systemctl(&["disable", "--now", name]);
            std::fs::remove_file(path)?;
            changed = true;
        }
    }
    if changed {
        systemctl(&["daemon-reload"])?;
    }
    let _ = std::fs::remove_dir_all(CACHE);
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
        assert!(update_unit(INSTALLED).contains("ExecStart=/usr/local/bin/remocto-service --update\n"));
        assert!(UPDATE_TIMER.contains("OnUnitActiveSec=6h\n"));
    }
}
