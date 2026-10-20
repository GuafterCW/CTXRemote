//! `ctxremote-service`: the Remocto host as a Windows service, and on Linux
//! as a systemd service without a desktop.

#[cfg(windows)]
mod win;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(windows)]
fn main() {
    std::process::exit(win::run());
}

#[cfg(target_os = "linux")]
fn main() {
    std::process::exit(linux::run());
}

#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    eprintln!("ctxremote-service läuft nur unter Windows und Linux.");
    std::process::exit(1);
}
