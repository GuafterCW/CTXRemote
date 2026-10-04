//! Secure Attention Sequence (Ctrl+Alt+Del), locking and restarting.

/// Sends Ctrl+Alt+Del. Windows honours it only from a service (with the
/// `SoftwareSASGeneration` policy set to 1 or 3) and silently ignores it elsewhere.
#[cfg(windows)]
pub fn send() -> anyhow::Result<()> {
    // SAFETY: plain call without pointers; no return value.
    unsafe { windows::Win32::Security::Authentication::Identity::SendSAS(false) };
    Ok(())
}

/// Locks the interactive session. Injected Win+L is ignored by Windows, so it
/// needs its own call from a process on the interactive desktop.
#[cfg(windows)]
pub fn lock() -> anyhow::Result<()> {
    // SAFETY: plain call without pointers.
    unsafe { windows::Win32::System::Shutdown::LockWorkStation() }?;
    Ok(())
}

/// Restarts the computer, closing applications without asking. Works from the
/// service (SYSTEM) and from a signed-in user who may shut down.
#[cfg(windows)]
pub fn restart() -> anyhow::Result<()> {
    use anyhow::Context;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Shutdown::{
        InitiateSystemShutdownExW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
        SHTDN_REASON_MINOR_OTHER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)?;
        let mut luid = LUID::default();
        LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid)?;
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        let adjusted = AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None);
        let _ = windows::Win32::Foundation::CloseHandle(token);
        adjusted.context("Recht zum Herunterfahren fehlt")?;
        InitiateSystemShutdownExW(
            PCWSTR::null(),
            PCWSTR::null(),
            0,
            true,
            true,
            SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED,
        )
        .context("Neustart wurde abgelehnt")?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn restart() -> anyhow::Result<()> {
    anyhow::bail!("Neustart wird nur unter Windows unterstützt")
}

#[cfg(not(windows))]
pub fn send() -> anyhow::Result<()> {
    anyhow::bail!("Strg+Alt+Entf wird nur unter Windows unterstützt")
}

#[cfg(not(windows))]
pub fn lock() -> anyhow::Result<()> {
    anyhow::bail!("Sperren wird nur unter Windows unterstützt")
}
