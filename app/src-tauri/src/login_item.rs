//! macOS: with unattended access on, the app starts at login (in the tray), so
//! the Mac can be reached without anyone opening it. A LaunchAgent in
//! `~/Library/LaunchAgents`, removed again when unattended access is turned off.

use std::path::PathBuf;

const LABEL: &str = "info.philipp-dev.ctxremote";

fn plist_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Registers or removes the login item. An app still on the disk image
/// (`/Volumes/…`) is not registered: that path is gone after ejecting.
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
