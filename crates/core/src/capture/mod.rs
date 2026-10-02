//! Screen capture backends.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{displays, Capturer};

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
pub use self::unsupported::{displays, Capturer};

/// Whether this platform can be controlled remotely (it can always view).
pub const HOST_SUPPORTED: bool = cfg!(windows);

#[cfg(not(windows))]
const UNSUPPORTED: &str = "Dieses Betriebssystem kann noch nicht ferngesteuert werden";

/// A monitor in virtual-desktop coordinates (physical pixels).
#[derive(Debug, Clone)]
pub struct Display {
    pub index: u8,
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl From<&Display> for ctxremote_proto::session::DisplayInfo {
    fn from(d: &Display) -> Self {
        Self {
            index: d.index,
            name: d.name.clone(),
            width: d.width,
            height: d.height,
            primary: d.primary,
        }
    }
}
