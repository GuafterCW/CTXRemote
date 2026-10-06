//! With unattended access on, the app starts at login (in the tray), so the
//! computer can be reached without anyone opening it; removed again when
//! unattended access is turned off. On a Mac a LaunchAgent in
//! `~/Library/LaunchAgents`, on Linux an XDG autostart entry in
//! `~/.config/autostart`.

use std::path::PathBuf;

const LABEL: &str = "info.philipp-dev.ctxremote";

#[cfg(target_os = "macos")]
fn plist_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Registers or removes the login item. An app still on the disk image
/// (`/Volumes/…`) is not registered: that path is gone after ejecting.
#[cfg(target_os = "macos")]
pub fn sync(enabled: bool) {
    let Some(path) = plist_path() else { return };
    let exe = std::env::current_exe().ok().filter(|p| !p.starts_with("/Volumes"));
    match exe {
        Some(exe) if enabled => {
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--tray</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#,
                escape(&exe.to_string_lossy())
            );
            if std::fs::read_to_string(&path).ok().as_deref() == Some(plist.as_str()) {
                return;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, plist) {
                tracing::warn!("Anmeldeobjekt nicht gespeichert: {e}");
            }
        }
        _ => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(target_os = "linux")]
fn autostart_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join(format!("{LABEL}.desktop")))
}

/// The autostart entry for `exe`. `Exec` is escaped twice, as the Desktop
/// Entry spec says: for the quoted argument, then for the string value.
#[cfg(any(target_os = "linux", test))]
fn desktop_entry(exe: &str) -> String {
    let mut arg = String::new();
    for c in exe.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            arg.push('\\');
        }
        arg.push(c);
    }
    let value = arg.replace('\\', "\\\\").replace('%', "%%");
    format!(
        "[Desktop Entry]\nType=Application\nName=CTXRemote\nComment=Fernwartung, im Hintergrund erreichbar\nExec=\"{value}\" --tray\nIcon=ctxremote\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(target_os = "linux")]
pub fn sync(enabled: bool) {
    let Some(path) = autostart_path() else { return };
    // An AppImage runs from a temporary mount: its own file is started instead.
    let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok());
    match exe {
        Some(exe) if enabled => {
            let entry = desktop_entry(&exe.to_string_lossy());
            if std::fs::read_to_string(&path).ok().as_deref() == Some(entry.as_str()) {
                return;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, entry) {
                tracing::warn!("Autostart nicht gespeichert: {e}");
            }
        }
        _ => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::desktop_entry;

    #[test]
    fn exec_line_is_escaped_twice() {
        let entry = desktop_entry("/opt/My Apps/ctx\"remote$1 50%");
        // Quoting gives \" and \$, the string value doubles the backslashes.
        assert!(entry.contains("Exec=\"/opt/My Apps/ctx\\\\\"remote\\\\$1 50%%\" --tray\n"), "{entry}");
        assert!(desktop_entry("/usr/bin/ctxremote").contains("Exec=\"/usr/bin/ctxremote\" --tray\n"));
    }
}
