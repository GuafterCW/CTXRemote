//! Keyboard and mouse injection backends.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::Injector;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::{accessibility_permission, Injector};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "macos")))]
pub use self::unsupported::Injector;
