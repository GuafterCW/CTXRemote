//! Runs the embedded NSIS installer silently and elevated, and finds the
//! installed app. Elsewhere than on Windows, and without an embedded
//! installer, installing is only simulated (for previews while developing).

use std::path::PathBuf;

/// The NSIS installer built by `tauri build`, embedded by build.rs; empty in
/// development builds.
pub static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.exe"));

pub const VERSION: &str = match option_env!("CTXREMOTE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug)]
pub enum Error {
    /// The user said no to the administrator prompt.
    #[cfg_attr(not(windows), allow(dead_code))]
    Declined,
    Failed(String),
}

/// What an NSIS exit code means for the user.
fn describe(code: u32) -> String {
    match code {
        1 => "Die Installation wurde abgebrochen.".into(),
        2 => "Das Installationsprogramm hat abgebrochen. Ist vielleicht schon eine neuere Version installiert?".into(),
        code => format!("Das Installationsprogramm meldet den Fehlercode {code}."),
    }
}

/// Compares dotted version numbers; anything unreadable counts as older.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(a) > parse(b)
}

/// Writes the installer to a temporary folder, runs it with `/S` and waits.
/// `parent` is the window the administrator prompt belongs to (0 for none).
pub fn run(parent: isize) -> Result<(), Error> {
    if PAYLOAD.is_empty() || !cfg!(windows) {
        std::thread::sleep(std::time::Duration::from_secs(5));
        return Ok(());
    }
    let dir = std::env::temp_dir().join(format!("ctxremote-setup-{}", std::process::id()));
    let path = dir.join("CTXRemote-Installer.exe");
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, PAYLOAD))
        .map_err(|e| Error::Failed(format!("Das Installationsprogramm konnte nicht entpackt werden: {e}")))?;
    let result = platform::run_elevated(&path, parent);
    let _ = std::fs::remove_dir_all(&dir);
    match result? {
        0 => Ok(()),
        code => Err(Error::Failed(describe(code))),
    }
}

/// The installed version, if CTXRemote is installed.
pub fn installed_version() -> Option<String> {
    platform::uninstall_value("DisplayVersion")
}

/// The installed app's executable.
pub fn app_path() -> Option<PathBuf> {
    let dir = platform::uninstall_value("InstallLocation")?;
    let exe = platform::uninstall_value("MainBinaryName").unwrap_or_else(|| "CTXRemote.exe".into());
    let path = PathBuf::from(dir.trim_matches('"')).join(exe.trim_matches('"'));
    path.exists().then_some(path)
}

/// Starts the installed app as the current user.
pub fn launch() {
    if let Some(path) = app_path() {
        platform::open(&path);
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND, WAIT_OBJECT_0};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, ShellExecuteW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};

    use super::Error;

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(Some(0)).collect()
    }

    /// Starts `path /S` through the administrator prompt and returns its exit code.
    pub fn run_elevated(path: &Path, parent: isize) -> Result<u32, Error> {
        let file = wide(path);
        let params = wide("/S");
        let verb = wide("runas");
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            hwnd: HWND(parent as *mut _),
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        unsafe {
            // ShellExecuteEx may use COM; ignoring "already initialized".
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            if let Err(e) = ShellExecuteExW(&mut info) {
                if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                    return Err(Error::Declined);
                }
                return Err(Error::Failed(format!("Das Installationsprogramm startet nicht: {}", e.message())));
            }
            if info.hProcess.is_invalid() {
                return Err(Error::Failed("Das Installationsprogramm startet nicht.".into()));
            }
            let waited = WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 0u32;
            let read = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            if waited != WAIT_OBJECT_0 || read.is_err() {
                return Err(Error::Failed("Das Ergebnis der Installation ist unbekannt.".into()));
            }
            Ok(code)
        }
    }

    fn read(root: HKEY, value: &str) -> Option<String> {
        let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\CTXRemote");
        let value = wide(value);
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        if status.is_err() {
            return None;
        }
        let chars = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..chars]).trim_end_matches('\0').trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// A value from CTXRemote's uninstall entry (machine-wide, else per user).
    pub fn uninstall_value(value: &str) -> Option<String> {
        read(HKEY_LOCAL_MACHINE, value).or_else(|| read(HKEY_CURRENT_USER, value))
    }

    pub fn open(path: &Path) {
        let file = wide(path);
        let dir = path.parent().map(wide);
        let verb = wide("open");
        unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
                SW_SHOWNORMAL,
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use super::Error;

    pub fn run_elevated(_path: &Path, _parent: isize) -> Result<u32, Error> {
        Ok(0)
    }

    pub fn uninstall_value(_value: &str) -> Option<String> {
        None
    }

    pub fn open(_path: &Path) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("0.1.20", "0.1.9"));
        assert!(!newer("0.1.9", "0.1.20"));
        assert!(!newer("0.1.19", "0.1.19"));
        assert!(newer("1.0.0", "0.9.99"));
    }
}
