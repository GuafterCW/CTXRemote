//! The shell in a pseudo console (ConPTY) on Windows: PowerShell as the user
//! signed in at the computer. In service mode the agent runs as SYSTEM and
//! starts it with that user's token, so a viewer gets no more than the user
//! could do at the keyboard (as with files, see `files::user`).
//!
//! ConPTY needs Windows 10 1809. Its functions are looked up at run time, so
//! the program still starts on older versions and only the terminal fails.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use windows::core::{s, w, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcessId,
    InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
};

use super::{Control, Started};

#[repr(C)]
#[derive(Clone, Copy)]
struct Coord {
    x: i16,
    y: i16,
}

type Hpcon = isize;
type CreateFn = unsafe extern "system" fn(Coord, HANDLE, HANDLE, u32, *mut Hpcon) -> HRESULT;
type ResizeFn = unsafe extern "system" fn(Hpcon, Coord) -> HRESULT;
type CloseFn = unsafe extern "system" fn(Hpcon);

struct ConPty {
    create: CreateFn,
    resize: ResizeFn,
    close: CloseFn,
}

fn conpty() -> Result<ConPty> {
    unsafe {
        let kernel = GetModuleHandleW(w!("kernel32.dll"))?;
        let (Some(create), Some(resize), Some(close)) = (
            GetProcAddress(kernel, s!("CreatePseudoConsole")),
            GetProcAddress(kernel, s!("ResizePseudoConsole")),
            GetProcAddress(kernel, s!("ClosePseudoConsole")),
        ) else {
            bail!("Das Terminal braucht Windows 10 ab Version 1809");
        };
        Ok(ConPty {
            create: std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateFn>(create),
            resize: std::mem::transmute::<unsafe extern "system" fn() -> isize, ResizeFn>(resize),
            close: std::mem::transmute::<unsafe extern "system" fn() -> isize, CloseFn>(close),
        })
    }
}

/// The pseudo console, closed exactly once: by the viewer (drop) or when
/// the shell ends by itself, since ConPTY keeps the output open until then.
struct Console {
    handle: Mutex<Option<Hpcon>>,
    close: CloseFn,
}

impl Console {
    fn close(&self) {
        if let Some(handle) = self.handle.lock().unwrap().take() {
            unsafe { (self.close)(handle) };
        }
    }
}

struct Pty {
    console: Arc<Console>,
    resize: ResizeFn,
    process: Arc<OwnedHandle>,
}

impl Control for Pty {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let Some(handle) = *self.console.handle.lock().unwrap() else { return Ok(()) };
        unsafe { (self.resize)(handle, coord(cols, rows)) }.ok()?;
        Ok(())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = unsafe { TerminateProcess(HANDLE(self.process.as_raw_handle()), 1) };
        // Closing can wait for the output to be read; not on the agent's loop.
        let console = self.console.clone();
        let _ = std::thread::Builder::new().name("ctxremote-terminal-close".into()).spawn(move || console.close());
    }
}

fn coord(cols: u16, rows: u16) -> Coord {
    Coord { x: cols.min(i16::MAX as u16) as i16, y: rows.min(i16::MAX as u16) as i16 }
}

fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0).context("Keine Pipe für das Terminal")?;
        Ok((OwnedHandle::from_raw_handle(read.0), OwnedHandle::from_raw_handle(write.0)))
    }
}

/// The signed-in user's token when running as SYSTEM; `None` when this
/// process already runs as the user.
fn user_token() -> Result<Option<OwnedHandle>> {
    if !crate::files::running_as_system()? {
        return Ok(None);
    }
    unsafe {
        let mut session = 0;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session)?;
        let mut token = HANDLE::default();
        if WTSQueryUserToken(session, &mut token).is_err() {
            bail!("Das Terminal ist erst verfügbar, wenn ein Benutzer angemeldet ist");
        }
        Ok(Some(OwnedHandle::from_raw_handle(token.0)))
    }
}

/// `NAME=value` from a Unicode environment block.
fn variable(block: *const u16, name: &str) -> Option<String> {
    let prefix: Vec<u16> = format!("{name}=").encode_utf16().collect();
    let mut at = block;
    unsafe {
        loop {
            let len = (0..).take_while(|&i| *at.add(i) != 0).count();
            if len == 0 {
                return None;
            }
            let entry = std::slice::from_raw_parts(at, len);
            if entry.len() >= prefix.len()
                && String::from_utf16_lossy(&entry[..prefix.len()]).eq_ignore_ascii_case(&String::from_utf16_lossy(&prefix))
            {
                return Some(String::from_utf16_lossy(&entry[prefix.len()..]));
            }
            at = at.add(len + 1);
        }
    }
}

/// Frees the environment block however `start` ends.
struct Environment(*mut c_void);

impl Drop for Environment {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { DestroyEnvironmentBlock(self.0) };
        }
    }
}

pub fn start(cols: u16, rows: u16) -> Result<Started> {
    let api = conpty()?;
    let token = user_token()?;
    let token_handle = token.as_ref().map(|t| HANDLE(t.as_raw_handle()));

    // The user's own environment (USERPROFILE, PATH, …), not SYSTEM's.
    let mut environment = Environment(std::ptr::null_mut());
    unsafe { CreateEnvironmentBlock(&mut environment.0, token_handle, false) }
        .context("Umgebung des Benutzers nicht lesbar")?;
    let block = environment.0 as *const u16;
    let user = variable(block, "USERNAME").unwrap_or_else(whoami::username);
    let home = variable(block, "USERPROFILE");
    let root = variable(block, "SystemRoot").unwrap_or_else(|| r"C:\Windows".into());

    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = pipe()?;
    let mut handle: Hpcon = 0;
    unsafe {
        (api.create)(
            coord(cols, rows),
            HANDLE(input_read.as_raw_handle()),
            HANDLE(output_write.as_raw_handle()),
            0,
            &mut handle,
        )
    }
    .ok()
    .context("Pseudo-Konsole nicht verfügbar")?;
    let console = Arc::new(Console { handle: Mutex::new(Some(handle)), close: api.close });
    // The console holds its own copies now.
    drop((input_read, output_write));

    let process = spawn(&console, token_handle, &environment, &root, home.as_deref())?;
    let process = Arc::new(process);

    // When the shell exits by itself, closing the console ends the output.
    let (watched, watcher_console) = (process.clone(), console.clone());
    let watcher = std::thread::Builder::new().name("ctxremote-terminal-wait".into()).spawn(move || {
        unsafe { WaitForSingleObject(HANDLE(watched.as_raw_handle()), INFINITE) };
        watcher_console.close();
    });
    if let Err(e) = watcher {
        tracing::warn!("Terminal wird beim Beenden nicht erkannt: {e}");
    }

    let host = whoami::devicename();
    Ok(Started {
        input: Box::new(File::from(input_write)),
        output: Box::new(File::from(output_read)),
        control: Box::new(Pty { console, resize: api.resize, process }),
        // The watcher closes the console; nothing else to collect.
        reap: Box::new(|| {}),
        what: format!("PowerShell als {user} auf {host}"),
    })
}

fn spawn(
    console: &Console,
    token: Option<HANDLE>,
    environment: &Environment,
    root: &str,
    home: Option<&str>,
) -> Result<OwnedHandle> {
    let handle = console.handle.lock().unwrap().expect("open until returned");
    unsafe {
        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        // Pointer-aligned, as the list holds pointers.
        let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)?;
        let result = (|| -> Result<OwnedHandle> {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(handle as *const c_void),
                std::mem::size_of::<Hpcon>(),
                None,
                None,
            )?;
            let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(Some(0)).collect();
            let startup = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                    lpDesktop: PWSTR(desktop.as_mut_ptr()),
                    // No handles of ours: the shell must use the console.
                    dwFlags: STARTF_USESTDHANDLES,
                    ..Default::default()
                },
                lpAttributeList: list,
            };
            let mut command: Vec<u16> =
                format!("\"{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoLogo")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
            let cwd: Option<Vec<u16>> = home.map(|h| h.encode_utf16().chain(Some(0)).collect());
            let cwd = cwd.as_ref().map_or(PCWSTR::null(), |c| PCWSTR(c.as_ptr()));
            let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;
            let mut info = PROCESS_INFORMATION::default();
            match token {
                Some(token) => CreateProcessAsUserW(
                    Some(token),
                    PCWSTR::null(),
                    Some(PWSTR(command.as_mut_ptr())),
                    None,
                    None,
                    false,
                    flags,
                    Some(environment.0),
                    cwd,
                    &startup.StartupInfo,
                    &mut info,
                ),
                None => CreateProcessW(
                    PCWSTR::null(),
                    Some(PWSTR(command.as_mut_ptr())),
                    None,
                    None,
                    false,
                    flags,
                    Some(environment.0),
                    cwd,
                    &startup.StartupInfo,
                    &mut info,
                ),
            }
            .context("PowerShell konnte nicht gestartet werden")?;
            drop(OwnedHandle::from_raw_handle(info.hThread.0));
            Ok(OwnedHandle::from_raw_handle(info.hProcess.0))
        })();
        DeleteProcThreadAttributeList(list);
        result
    }
}
