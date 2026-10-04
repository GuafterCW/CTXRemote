//! Checking for, fetching and starting client updates from the server.
//!
//! Releases are signed in the pipeline; [`PUBLIC_KEY`] is built into the
//! client from `CTXREMOTE_UPDATE_KEY`. Builds without it (local development)
//! never update. See `docs/DEPLOY.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::{ClientMsg, ServerMsg};
use ctxremote_proto::update::{is_newer, UpdateInfo};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

use crate::net;

/// The platform name releases are published under.
pub const PLATFORM: &str = if cfg!(all(windows, target_arch = "x86_64")) {
    "windows-x86_64"
} else if cfg!(all(windows, target_arch = "aarch64")) {
    "windows-aarch64"
} else {
    "unsupported"
};

/// This build's version: set by the release pipeline, else the crate version.
pub const VERSION: &str = match option_env!("CTXREMOTE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Hex-encoded Ed25519 key that releases must be signed with.
pub const PUBLIC_KEY: Option<&str> = option_env!("CTXREMOTE_UPDATE_KEY");

const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether this build can update itself at all.
pub fn enabled() -> bool {
    PLATFORM != "unsupported" && public_key().is_some()
}

fn public_key() -> Option<[u8; 32]> {
    hex::decode(PUBLIC_KEY?.trim()).ok()?.try_into().ok()
}

/// Asks `server` for a release newer than this build. `None` if there is none,
/// the server predates updates, or this build cannot update.
pub async fn check(server: &str) -> Result<Option<UpdateInfo>> {
    match public_key() {
        Some(key) if enabled() => check_with(server, PLATFORM, VERSION, &key).await,
        _ => Ok(None),
    }
}

/// [`check`] with everything spelled out, for tests.
pub async fn check_with(server: &str, platform: &str, current: &str, key: &[u8; 32]) -> Result<Option<UpdateInfo>> {
    let (mut t, _) = net::dial(server).await?;
    framing::send(&mut t, &ClientMsg::UpdateCheck { platform: platform.into() }).await?;
    // Older servers do not know the request and hang up: no update.
    let info = match timeout(STEP_TIMEOUT, framing::recv::<ServerMsg>(&mut t)).await? {
        Ok(ServerMsg::Update(info)) => info,
        Ok(other) => bail!("unerwartete Serverantwort: {other:?}"),
        Err(_) => return Ok(None),
    };
    let Some(info) = info else { return Ok(None) };
    if info.platform != platform || !is_newer(&info.version, current) {
        return Ok(None);
    }
    if !info.verify(key) {
        bail!("Das angebotene Update ist nicht gültig signiert und wird ignoriert");
    }
    Ok(Some(info))
}

/// Downloads `info`'s installer into `dir` and checks it against the signed
/// size and hash. Returns the installer's path.
pub async fn download(server: &str, info: &UpdateInfo, dir: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let path = dir.join(format!("CTXRemote-{}-setup.exe", info.version));
    let partial = path.with_extension("part");
    let result = fetch(server, info, &partial).await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    tokio::fs::rename(&partial, &path).await?;
    Ok(path)
}

async fn fetch(server: &str, info: &UpdateInfo, partial: &Path) -> Result<()> {
    let (mut t, _) = net::dial(server).await?;
    framing::send(&mut t, &ClientMsg::UpdateDownload { platform: info.platform.clone(), version: info.version.clone() })
        .await?;
    let mut file = tokio::fs::File::create(partial).await?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    loop {
        let frame = timeout(STEP_TIMEOUT, t.next())
            .await
            .context("Der Server liefert das Update nicht weiter")?
            .context("Verbindung beim Herunterladen getrennt")??;
        match framing::decode::<ServerMsg>(&frame)? {
            ServerMsg::UpdateData(data) => {
                received += data.len() as u64;
                if received > info.size {
                    bail!("Das Update ist größer als angekündigt");
                }
                hash.update(&data);
                file.write_all(&data).await?;
            }
            ServerMsg::UpdateEnd => break,
            ServerMsg::Error(e) => return Err(e.into()),
            other => bail!("unerwartete Serverantwort: {other:?}"),
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    // The signature covers size and hash, so this proves the file is the signed one.
    let digest: [u8; 32] = hash.finalize().into();
    if received != info.size || digest != info.sha256 {
        bail!("Das heruntergeladene Update stimmt nicht mit der Signatur überein");
    }
    Ok(())
}

/// How the installer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// From the service (SYSTEM): no window, no questions.
    Silent,
    /// From the app: asks for administrator rights, shows progress.
    Interactive,
}

/// Starts the installer and returns at once; it outlives this process, which
/// it stops and replaces.
pub fn launch(installer: &Path, mode: InstallMode) -> Result<()> {
    imp::launch(installer, mode)
}

/// From the service (SYSTEM): starts `exe` as the user signed in at the
/// console, on their desktop, with their environment. Used to bring the app
/// back after a silent update closed it. Fails while nobody is signed in.
pub fn start_for_console_user(exe: &Path, args: &str) -> Result<()> {
    imp::start_for_console_user(exe, args)
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn launch(_: &Path, _: InstallMode) -> Result<()> {
        bail!("Updates gibt es nur unter Windows")
    }

    pub fn start_for_console_user(_: &Path, _: &str) -> Result<()> {
        bail!("Nur unter Windows")
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;

    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::*;

    /// Must match the name `ctxremote-service` registers.
    const SERVICE_NAME: &str = "CTXRemote";
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    pub fn start_for_console_user(exe: &Path, args: &str) -> Result<()> {
        unsafe {
            let session = WTSGetActiveConsoleSessionId();
            if session == u32::MAX {
                bail!("Keine Konsolensitzung aktiv");
            }
            let mut token = HANDLE::default();
            WTSQueryUserToken(session, &mut token).context("Niemand ist angemeldet")?;
            let token = OwnedHandle::from_raw_handle(token.0);
            // The user's own environment (APPDATA, TEMP, …), not SYSTEM's.
            let mut environment = std::ptr::null_mut();
            CreateEnvironmentBlock(&mut environment, Some(HANDLE(token.as_raw_handle())), false)
                .context("Umgebung des Benutzers nicht lesbar")?;

            let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(Some(0)).collect();
            let startup = STARTUPINFOW {
                cb: size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let app: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut command: Vec<u16> =
                format!("\"{}\" {args}", exe.display()).encode_utf16().chain(Some(0)).collect();
            let mut info = PROCESS_INFORMATION::default();
            let started = CreateProcessAsUserW(
                Some(HANDLE(token.as_raw_handle())),
                PCWSTR(app.as_ptr()),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_UNICODE_ENVIRONMENT,
                Some(environment),
                PCWSTR::null(),
                &startup,
                &mut info,
            );
            let _ = DestroyEnvironmentBlock(environment);
            started.context("Programm konnte nicht gestartet werden")?;
            let _ = CloseHandle(info.hThread);
            let _ = CloseHandle(info.hProcess);
            Ok(())
        }
    }

    pub fn launch(installer: &Path, mode: InstallMode) -> Result<()> {
        match mode {
            InstallMode::Silent => {
                // The installer stops the service. Whatever happens next (success or
                // an aborted install), `sc start` brings the service back, so the
                // device never stays unreachable. Detached and, where allowed, out of
                // the service's job, so stopping the service does not end the chain.
                let chain = format!("/S /C \"\"{}\" /S & sc.exe start {SERVICE_NAME}\"", installer.display());
                let spawn = |flags: u32| {
                    std::process::Command::new("cmd.exe").raw_arg(&chain).creation_flags(flags).spawn()
                };
                spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
                    .or_else(|_| spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP))
                    .context("Installer konnte nicht gestartet werden")?;
                Ok(())
            }
            InstallMode::Interactive => {
                let file: Vec<u16> = installer.as_os_str().encode_wide().chain(Some(0)).collect();
                let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
                // Passive: progress only, no questions; the installer closes the running app.
                let params: Vec<u16> = "/P /R".encode_utf16().chain(Some(0)).collect();
                // SAFETY: all strings are NUL-terminated and outlive the call.
                let result = unsafe {
                    ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR(params.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL)
                };
                // Values above 32 mean success; 5 is a declined UAC prompt.
                match result.0 as isize {
                    n if n > 32 => Ok(()),
                    5 => bail!("Die Installation wurde nicht bestätigt"),
                    n => bail!("Installer konnte nicht gestartet werden (Fehler {n})"),
                }
            }
        }
    }
}
