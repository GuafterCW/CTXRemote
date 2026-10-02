//! `ctxremote-service`: the CTXRemote host as a Windows service.

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    std::process::exit(win::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ctxremote-service läuft nur unter Windows.");
    std::process::exit(1);
}
