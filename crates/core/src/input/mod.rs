//! Keyboard and mouse injection backends.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::Injector;

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
pub use self::unsupported::Injector;
