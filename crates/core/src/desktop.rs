//! Following the input desktop: `Default` normally, `Winlogon` on the lock
//! screen, the sign-in screen and UAC prompts.
//!
//! Capture and input only reach the desktop their thread is attached to. Only a
//! process running as SYSTEM may attach to `Winlogon`; elsewhere the switch
//! fails quietly and the thread stays where it is.

#[cfg(windows)]
pub use imp::follow_input;

/// Attaches the calling thread to the current input desktop if it is not
/// already there. Returns `true` if the thread switched.
#[cfg(not(windows))]
pub fn follow_input() -> bool {
    false
}

#[cfg(windows)]
mod imp {
    use std::cell::Cell;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop,
        SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, HDESK, UOI_NAME,
    };
    use windows::Win32::System::Threading::GetCurrentThreadId;

    const GENERIC_ALL: u32 = 0x1000_0000;

    thread_local! {
        /// The desktop this thread was switched to; ours to close on the next switch.
        static ATTACHED: Cell<Option<isize>> = const { Cell::new(None) };
    }

    /// Attaches the calling thread to the current input desktop if it is not
    /// already there. Returns `true` if the thread switched.
    pub fn follow_input() -> bool {
        unsafe {
            let Ok(input) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(GENERIC_ALL))
            else {
                return false;
            };
            let current = GetThreadDesktop(GetCurrentThreadId()).ok();
            let same = current.is_some_and(|c| name(c).is_some() && name(c) == name(input));
            if same || SetThreadDesktop(input).is_err() {
                let _ = CloseDesktop(input);
                return false;
            }
            if let Some(previous) = ATTACHED.replace(Some(input.0 as isize)) {
                let _ = CloseDesktop(HDESK(previous as _));
            }
            tracing::debug!("Eingabedesktop gewechselt zu {:?}", name(input));
            true
        }
    }

    fn name(desktop: HDESK) -> Option<String> {
        let mut buf = [0u16; 64];
        let mut needed = 0;
        unsafe {
            GetUserObjectInformationW(
                HANDLE(desktop.0),
                UOI_NAME,
                Some(buf.as_mut_ptr().cast()),
                (buf.len() * 2) as u32,
                Some(&mut needed),
            )
            .ok()?;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}
