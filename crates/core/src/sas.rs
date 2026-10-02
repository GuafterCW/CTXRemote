//! Secure Attention Sequence (Ctrl+Alt+Del) and locking the workstation.

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

#[cfg(not(windows))]
pub fn send() -> anyhow::Result<()> {
    anyhow::bail!("Strg+Alt+Entf wird nur unter Windows unterstützt")
}

#[cfg(not(windows))]
pub fn lock() -> anyhow::Result<()> {
    anyhow::bail!("Sperren wird nur unter Windows unterstützt")
}
