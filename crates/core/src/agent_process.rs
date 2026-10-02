//! The screen side in a separate agent process, as the Windows service runs it.
//!
//! The service lives in session 0, which has no screen. For each session it
//! starts `<exe> --agent <pipe>` as SYSTEM in the console session and talks to
//! it over a named pipe that only that process can open. See
//! `docs/WINDOWS-SERVICE.md`.

use std::path::PathBuf;

use anyhow::Result;
use futures::future::BoxFuture;

use crate::host::{ScreenChannels, ScreenSource};

/// Starts `exe --agent <pipe>` for every session.
pub struct AgentProcess {
    exe: PathBuf,
}

impl AgentProcess {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe }
    }
}

impl ScreenSource for AgentProcess {
    fn open(&self) -> BoxFuture<'static, Result<ScreenChannels>> {
        let exe = self.exe.clone();
        Box::pin(async move { imp::open(exe).await })
    }
}

/// The agent process: connects to the service's pipe and serves the session.
pub async fn serve(pipe: &str) -> Result<()> {
    imp::serve(pipe).await
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub async fn open(_: PathBuf) -> Result<ScreenChannels> {
        anyhow::bail!("Agent-Prozesse gibt es nur unter Windows")
    }

    pub async fn serve(_: &str) -> Result<()> {
        anyhow::bail!("Agent-Prozesse gibt es nur unter Windows")
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::time::Duration;

    use anyhow::{bail, Context};
    use bytes::Bytes;
    use ctxremote_proto::framing::MAX_FRAME;
    use ctxremote_proto::session::{HostMsg, ViewerMsg};
    use futures::{SinkExt, StreamExt};
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use tokio::io::{AsyncRead, AsyncWrite};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use tokio::sync::{mpsc, oneshot};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};
    use tracing::{info, warn};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, SetTokenInformation,
        TokenPrimary, TokenSessionId, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY,
        TOKEN_DUPLICATE, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
        TerminateProcess, PROCESS_TERMINATE,
        CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    use super::*;

    const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    /// Granted by `SetTokenInformation`/`DuplicateTokenEx` callers asking for everything.
    const MAXIMUM_ALLOWED: u32 = 0x0200_0000;

    pub async fn open(exe: PathBuf) -> Result<ScreenChannels> {
        let name = format!(r"\\.\pipe\ctxremote-agent-{:016x}", rand::random::<u64>());
        let server = create_pipe(&name)?;
        let (process, pid) = spawn_agent(&exe, &name)?;

        match tokio::time::timeout(CONNECT_TIMEOUT, server.connect()).await {
            Ok(Ok(())) => {}
            result => {
                unsafe {
                    let _ = TerminateProcess(HANDLE(process.as_raw_handle()), 1);
                }
                match result {
                    Ok(Err(e)) => return Err(e).context("Agent konnte sich nicht verbinden"),
                    _ => bail!("Agent hat sich nicht rechtzeitig gemeldet"),
                }
            }
        }
        // Only the process we started may serve the screen.
        let mut client = 0;
        unsafe { GetNamedPipeClientProcessId(HANDLE(server.as_raw_handle()), &mut client) }?;
        if client != pid {
            bail!("Fremder Prozess {client} an der Agent-Pipe");
        }
        info!(pid, "Agent verbunden");
        let (to_agent, from_agent) = bridge::<HostMsg, ViewerMsg, _>(server);
        Ok((to_agent, from_agent))
    }

    pub async fn serve(pipe: &str) -> Result<()> {
        let client = ClientOptions::new().open(pipe).context("Agent-Pipe nicht erreichbar")?;
        let (outbox, inbox) = bridge::<ViewerMsg, HostMsg, _>(client);
        crate::agent::run(inbox, outbox).await
    }

    /// Frames `io` and moves messages between it and two channels. The pipe
    /// closes once the outgoing sender is dropped or the peer hangs up.
    fn bridge<In, Out, Io>(io: Io) -> (mpsc::Sender<Out>, mpsc::Receiver<In>)
    where
        In: DeserializeOwned + Send + 'static,
        Out: Serialize + Send + 'static,
        Io: AsyncRead + AsyncWrite + Send + 'static,
    {
        let codec = LengthDelimitedCodec::builder().max_frame_length(MAX_FRAME).new_codec();
        let (mut sink, mut stream) = Framed::new(io, codec).split();
        // Small outgoing queue: a slow link throttles capture instead of queueing frames.
        let (out_tx, mut out_rx) = mpsc::channel::<Out>(2);
        let (in_tx, in_rx) = mpsc::channel::<In>(64);
        let (done_tx, mut done_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                let Ok(bytes) = postcard::to_stdvec(&msg) else { break };
                if sink.send(Bytes::from(bytes)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
            let _ = done_tx.send(());
        });
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    frame = stream.next() => {
                        let Some(Ok(frame)) = frame else { break };
                        let Ok(msg) = postcard::from_bytes::<In>(&frame) else {
                            warn!("ungültige Nachricht an der Agent-Pipe");
                            break;
                        };
                        if in_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    // Both halves must go for the pipe to close.
                    _ = &mut done_rx => break,
                }
            }
        });
        (out_tx, in_rx)
    }

    /// A pipe only the current account (SYSTEM for the service) can open, local only.
    fn create_pipe(name: &str) -> Result<NamedPipeServer> {
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", current_user_sid()?);
        let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )?;
        }
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .max_instances(1)
                .create_with_security_attributes_raw(name, (&mut attrs as *mut SECURITY_ATTRIBUTES).cast())
        };
        unsafe {
            LocalFree(Some(HLOCAL(descriptor.0)));
        }
        server.context("Agent-Pipe konnte nicht angelegt werden")
    }

    fn current_user_sid() -> Result<String> {
        unsafe {
            let token = process_token(TOKEN_QUERY)?;
            let mut needed = 0;
            let _ = GetTokenInformation(HANDLE(token.as_raw_handle()), TokenUser, None, 0, &mut needed);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text)?;
            let sid = text.to_string();
            LocalFree(Some(HLOCAL(text.0.cast())));
            Ok(sid?)
        }
    }

    fn process_token(access: TOKEN_ACCESS_MASK) -> Result<OwnedHandle> {
        let mut token = HANDLE::default();
        unsafe {
            OpenProcessToken(GetCurrentProcess(), access, &mut token)?;
            Ok(OwnedHandle::from_raw_handle(token.0))
        }
    }

    /// Starts the agent in the console session, as SYSTEM with our own token.
    /// Outside the service (same session, e.g. `--console`), a plain child is enough.
    fn spawn_agent(exe: &std::path::Path, pipe: &str) -> Result<(OwnedHandle, u32)> {
        let console = unsafe { WTSGetActiveConsoleSessionId() };
        if console == u32::MAX {
            bail!("Keine Konsolensitzung aktiv");
        }
        let mut own = 0;
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut own) }?;
        if own == console {
            let child = std::process::Command::new(exe).arg("--agent").arg(pipe).spawn()?;
            let pid = child.id();
            let process = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }?;
            return Ok((unsafe { OwnedHandle::from_raw_handle(process.0) }, pid));
        }

        unsafe {
            let token = process_token(
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_SESSIONID | TOKEN_ADJUST_DEFAULT,
            )?;
            let mut primary = HANDLE::default();
            DuplicateTokenEx(
                HANDLE(token.as_raw_handle()),
                TOKEN_ACCESS_MASK(MAXIMUM_ALLOWED),
                None,
                SecurityImpersonation,
                TokenPrimary,
                &mut primary,
            )?;
            let primary = OwnedHandle::from_raw_handle(primary.0);
            // Needs SeTcbPrivilege, which SYSTEM holds.
            SetTokenInformation(
                HANDLE(primary.as_raw_handle()),
                TokenSessionId,
                (&console as *const u32).cast::<c_void>(),
                size_of::<u32>() as u32,
            )
            .context("Sitzung des Agenten konnte nicht gesetzt werden")?;

            let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(Some(0)).collect();
            let startup = STARTUPINFOW {
                cb: size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut command: Vec<u16> = format!("\"{}\" --agent {pipe}", exe.display())
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let app: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut info = PROCESS_INFORMATION::default();
            CreateProcessAsUserW(
                Some(HANDLE(primary.as_raw_handle())),
                PCWSTR(app.as_ptr()),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW,
                None,
                PCWSTR::null(),
                &startup,
                &mut info,
            )
            .context("Agent konnte nicht gestartet werden")?;
            let _ = CloseHandle(info.hThread);
            Ok((OwnedHandle::from_raw_handle(info.hProcess.0), info.dwProcessId))
        }
    }
}
