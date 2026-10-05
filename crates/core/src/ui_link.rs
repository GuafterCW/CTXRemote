//! The link between the app and the Windows service (`\\.\pipe\ctxremote-ui`).
//!
//! Every signed-in user may see the service's ID, one-time password and
//! sessions, renew the password and end a session. Changing settings needs an
//! elevated administrator, which the service checks on the client's token.
//! Messages are length-prefixed JSON.

use serde::{Deserialize, Serialize};

use crate::config::DirectSettings;
use crate::host::{HostEvent, Presence};

pub const PIPE: &str = r"\\.\pipe\ctxremote-ui";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    pub presence: Presence,
    pub password: String,
    pub server: String,
    pub unattended: bool,
    pub sessions: Vec<(u64, String)>,
    /// Profiles of the viewers in `sessions` that sent one.
    #[serde(default)]
    pub session_profiles: Vec<(u64, crate::profile::Profile)>,
    pub direct: DirectSettings,
    /// The listener for direct connections runs.
    pub direct_active: bool,
    /// Sessions whose viewer can receive chat messages.
    #[serde(default)]
    pub chat_sessions: Vec<u64>,
    /// The device's public alias, if it has one.
    #[serde(default)]
    pub public_alias: Option<String>,
    /// Devices of the account may connect without a password.
    #[serde(default)]
    pub account_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiRequest {
    RefreshPassword,
    EndSession(u64),
    /// As [`crate::config::Config::apply_settings`]; elevated administrators only.
    Configure { server: String, permanent_password: Option<String> },
    /// As [`crate::config::Config::apply_direct`]; elevated administrators only.
    /// Appended last: the JSON enum is matched by name, but keep the order anyway.
    ConfigureDirect(DirectSettings),
    /// A chat message to the viewer of a session; any signed-in user, like `EndSession`.
    Chat { session: u64, text: String },
    /// Sets or drops the public alias; any signed-in user, it only names the device.
    SetPublicAlias(Option<String>),
    /// Lets the account's devices in without a password (`None`: no longer);
    /// elevated administrators only, as it grants unattended access.
    ConfigureAccountAccess(Option<crate::account::AccessGrant>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiEvent {
    State(ServiceState),
    Host(HostEvent),
    /// The answer to `Configure`.
    Configured(Result<(), String>),
    /// The answer to `SetPublicAlias`: the alias as stored.
    AliasSet(Result<Option<String>, String>),
}

#[cfg(windows)]
pub use imp::{open_pipe, serve, ServiceLink};

#[cfg(not(windows))]
pub struct ServiceLink;

#[cfg(not(windows))]
impl ServiceLink {
    pub async fn connect(_: impl Fn(Option<UiEvent>) + Send + 'static) -> anyhow::Result<Self> {
        anyhow::bail!("Den CTXRemote-Dienst gibt es nur unter Windows")
    }

    pub fn send(&self, _: UiRequest) {}
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::{Arc, RwLock};

    use anyhow::{bail, Context, Result};
    use bytes::Bytes;
    use futures::{SinkExt, StreamExt};
    use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
    use tokio::sync::{broadcast, mpsc, watch};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};
    use tracing::{info, warn};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        CheckTokenMembership, CreateWellKnownSid, RevertToSelf, WinBuiltinAdministratorsSid,
        PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    };
    use windows::Win32::System::Pipes::{GetNamedPipeServerProcessId, ImpersonateNamedPipeClient};
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_MANAGER_CONNECT,
        SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_STATUS_PROCESS,
    };
    use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

    use super::*;
    use crate::config::Config;
    use crate::host::Host;

    /// SYSTEM, Administrators and the creating account (owner): everything.
    /// Signed-in users: read and write data, but not FILE_CREATE_PIPE_INSTANCE,
    /// so nobody can squat the name.
    const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;0x12008b;;;AU)";
    /// FILE_GENERIC_READ | FILE_WRITE_DATA, the users' share of [`PIPE_SDDL`].
    const USER_ACCESS: u32 = 0x0012_008b;
    const MAX_MESSAGE: usize = 64 * 1024;

    type Link<T> = Framed<T, LengthDelimitedCodec>;

    fn framed<T: tokio::io::AsyncRead + tokio::io::AsyncWrite>(io: T) -> Link<T> {
        Framed::new(io, LengthDelimitedCodec::builder().max_frame_length(MAX_MESSAGE).new_codec())
    }

    fn encode<T: Serialize>(msg: &T) -> Bytes {
        Bytes::from(serde_json::to_vec(msg).expect("UI messages serialize"))
    }

    // ------------------------------------------------------------- service

    /// Serves the app until the process ends. Runs inside the service.
    pub async fn serve(host: Host, config: Arc<RwLock<Config>>) -> Result<()> {
        // Bumped after a settings change, so every client gets the new state.
        let changed = Arc::new(watch::Sender::new(()));
        let mut server = create_pipe(true)?;
        loop {
            if let Err(e) = server.connect().await {
                warn!("UI-Pipe: {e}");
                server = create_pipe(false)?;
                continue;
            }
            let connected = std::mem::replace(&mut server, create_pipe(false)?);
            tokio::spawn(client(connected, host.clone(), config.clone(), changed.clone()));
        }
    }

    fn create_pipe(first: bool) -> Result<NamedPipeServer> {
        let sddl: Vec<u16> = PIPE_SDDL.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )?;
            let mut attrs = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            // The first instance fails if someone already holds the name.
            let server = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(PIPE, (&mut attrs as *mut SECURITY_ATTRIBUTES).cast());
            LocalFree(Some(HLOCAL(descriptor.0)));
            server.context("UI-Pipe konnte nicht angelegt werden")
        }
    }

    fn state(host: &Host, config: &RwLock<Config>) -> ServiceState {
        let config = config.read().unwrap();
        ServiceState {
            presence: host.presence().borrow().clone(),
            password: host.password(),
            server: config.server.clone(),
            unattended: config.permanent_password.as_deref().is_some_and(|p| !p.is_empty()),
            sessions: host.sessions(),
            session_profiles: host.session_profiles(),
            chat_sessions: host.chat_sessions(),
            public_alias: config.public_alias.clone(),
            account_access: config.account_access.as_ref().is_some_and(|g| Some(g.host) == config.device_id),
            direct: config.direct_settings(),
            direct_active: host.direct_active(),
        }
    }

    async fn client(
        pipe: NamedPipeServer,
        host: Host,
        config: Arc<RwLock<Config>>,
        changed: Arc<watch::Sender<()>>,
    ) {
        // Stays valid while `link` owns the pipe.
        let handle = pipe.as_raw_handle() as isize;
        let mut link = framed(pipe);
        let mut presence = host.presence();
        let mut events = host.events();
        let mut changed_rx = changed.subscribe();
        if link.send(encode(&UiEvent::State(state(&host, &config)))).await.is_err() {
            return;
        }
        loop {
            let out = tokio::select! {
                frame = link.next() => {
                    let Some(Ok(frame)) = frame else { return };
                    let Ok(request) = serde_json::from_slice::<UiRequest>(&frame) else {
                        warn!("UI-Pipe: ungültige Anfrage");
                        return;
                    };
                    match request {
                        UiRequest::RefreshPassword => { host.refresh_password(); continue; }
                        UiRequest::EndSession(session) => { host.end_session(session); continue; }
                        UiRequest::SetPublicAlias(alias) => {
                            let answer = host.set_public_alias(alias).await;
                            if answer.is_ok() {
                                changed.send_replace(());
                            }
                            UiEvent::AliasSet(answer)
                        }
                        UiRequest::Chat { session, text } => {
                            if let Err(e) = host.send_chat(session, &text) {
                                tracing::debug!("Chat nicht zugestellt: {e}");
                            }
                            continue;
                        }
                        UiRequest::Configure { server, permanent_password } => {
                            let answer = if is_elevated_admin(HANDLE(handle as _)) {
                                configure(&host, &config, &server, permanent_password.as_deref())
                            } else {
                                Err("Nur Administratoren dürfen die Einstellungen des Dienstes ändern".into())
                            };
                            if answer.is_ok() {
                                changed.send_replace(());
                            }
                            UiEvent::Configured(answer)
                        }
                        UiRequest::ConfigureAccountAccess(grant) => {
                            let answer = if is_elevated_admin(HANDLE(handle as _)) {
                                configure_account_access(&config, grant)
                            } else {
                                Err("Nur Administratoren dürfen die Einstellungen des Dienstes ändern".into())
                            };
                            if answer.is_ok() {
                                changed.send_replace(());
                            }
                            UiEvent::Configured(answer)
                        }
                        UiRequest::ConfigureDirect(settings) => {
                            let answer = if is_elevated_admin(HANDLE(handle as _)) {
                                configure_direct(&host, &config, &settings)
                            } else {
                                Err("Nur Administratoren dürfen die Einstellungen des Dienstes ändern".into())
                            };
                            if answer.is_ok() {
                                // The listener restarts in the background; let it settle so the state is current.
                                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                                changed.send_replace(());
                            }
                            UiEvent::Configured(answer)
                        }
                    }
                }
                _ = presence.changed() => UiEvent::State(state(&host, &config)),
                _ = changed_rx.changed() => UiEvent::State(state(&host, &config)),
                event = events.recv() => match event {
                    Ok(event) => {
                        if link.send(encode(&UiEvent::Host(event))).await.is_err() {
                            return;
                        }
                        UiEvent::State(state(&host, &config))
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => UiEvent::State(state(&host, &config)),
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            };
            if link.send(encode(&out)).await.is_err() {
                return;
            }
        }
    }

    fn configure(
        host: &Host,
        config: &RwLock<Config>,
        server: &str,
        permanent_password: Option<&str>,
    ) -> Result<(), String> {
        let server_changed = {
            let mut config = config.write().unwrap();
            let changed = config.apply_settings(server, permanent_password).map_err(|e| e.to_string())?;
            config.save().map_err(|e| format!("Einstellungen nicht gespeichert: {e:#}"))?;
            changed
        };
        info!("Einstellungen des Dienstes geändert");
        if server_changed {
            host.reconnect();
        }
        Ok(())
    }

    fn configure_direct(host: &Host, config: &RwLock<Config>, settings: &DirectSettings) -> Result<(), String> {
        let changed = {
            let mut config = config.write().unwrap();
            let changed = config.apply_direct(settings).map_err(|e| e.to_string())?;
            config.save().map_err(|e| format!("Einstellungen nicht gespeichert: {e:#}"))?;
            changed
        };
        info!("Direktverbindungs-Einstellungen des Dienstes geändert");
        if changed {
            host.set_direct(settings.clone());
        }
        Ok(())
    }

    fn configure_account_access(config: &RwLock<Config>, grant: Option<crate::account::AccessGrant>) -> Result<(), String> {
        let mut config = config.write().unwrap();
        if let Some(grant) = &grant {
            if Some(grant.host) != config.device_id || grant.witness_key().is_none() || grant.password.len() != 64 {
                return Err("Die Freigabe passt nicht zu diesem Gerät".into());
            }
        }
        let open = grant.is_some();
        config.account_access = grant;
        config.save().map_err(|e| format!("Einstellungen nicht gespeichert: {e:#}"))?;
        info!(open, "Zugriff für Geräte des Kontos geändert");
        Ok(())
    }

    /// Whether the pipe's client runs with an elevated administrator token.
    /// A filtered (non-elevated) admin token has Administrators as deny-only and fails.
    fn is_elevated_admin(pipe: HANDLE) -> bool {
        unsafe {
            if ImpersonateNamedPipeClient(pipe).is_err() {
                return false;
            }
            let mut token = HANDLE::default();
            let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token).is_ok();
            if RevertToSelf().is_err() {
                // Never keep running with the client's identity.
                std::process::abort();
            }
            if !opened {
                return false;
            }
            let token = OwnedHandle::from_raw_handle(token.0);
            let mut sid = [0u8; 68];
            let mut size = sid.len() as u32;
            let sid = PSID(sid.as_mut_ptr().cast());
            if CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(sid), &mut size).is_err() {
                return false;
            }
            let mut member = false.into();
            CheckTokenMembership(Some(HANDLE(token.as_raw_handle())), sid, &mut member).is_ok()
                && member.as_bool()
        }
    }

    // --------------------------------------------------------------- app

    /// The app's end of the link.
    pub struct ServiceLink {
        requests: mpsc::UnboundedSender<UiRequest>,
    }

    impl ServiceLink {
        /// Connects to the running service. `on_event` runs on a network task and
        /// gets `None` once the link is gone (service stopped).
        pub async fn connect(on_event: impl Fn(Option<UiEvent>) + Send + 'static) -> Result<Self> {
            let pipe = open_pipe().context("CTXRemote-Dienst nicht erreichbar")?;
            // Only the real service may answer, not a process that took the name.
            let mut server = 0;
            unsafe { GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut server) }?;
            if Some(server) != service_pid() {
                bail!("Unerwarteter Prozess {server} an der Dienst-Pipe");
            }

            let (mut sink, mut stream) = framed(pipe).split();
            let (requests, mut outgoing) = mpsc::unbounded_channel::<UiRequest>();
            tokio::spawn(async move {
                while let Some(request) = outgoing.recv().await {
                    if sink.send(encode(&request)).await.is_err() {
                        break;
                    }
                }
            });
            tokio::spawn(async move {
                while let Some(Ok(frame)) = stream.next().await {
                    match serde_json::from_slice::<UiEvent>(&frame) {
                        Ok(event) => on_event(Some(event)),
                        Err(e) => {
                            warn!("Dienst-Pipe: ungültige Nachricht: {e}");
                            break;
                        }
                    }
                }
                on_event(None);
            });
            Ok(Self { requests })
        }

        pub fn send(&self, request: UiRequest) {
            let _ = self.requests.send(request);
        }
    }

    /// Opens the pipe with exactly the rights the DACL grants users. Tokio's
    /// `ClientOptions` asks for GENERIC_WRITE, which includes creating instances.
    #[doc(hidden)]
    pub fn open_pipe() -> Result<NamedPipeClient> {
        let name: Vec<u16> = PIPE.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let handle = CreateFileW(
                PCWSTR(name.as_ptr()),
                USER_ACCESS,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )?;
            Ok(NamedPipeClient::from_raw_handle(handle.0)?)
        }
    }

    /// Process ID of the running CTXRemote service.
    fn service_pid() -> Option<u32> {
        unsafe {
            let manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).ok()?;
            let service = OpenServiceW(manager, w!("CTXRemote"), SERVICE_QUERY_STATUS);
            let _ = CloseServiceHandle(manager);
            let service = service.ok()?;
            let mut status = SERVICE_STATUS_PROCESS::default();
            let mut needed = 0;
            let buffer = std::slice::from_raw_parts_mut(
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
                size_of::<SERVICE_STATUS_PROCESS>(),
            );
            let queried = QueryServiceStatusEx(service, SC_STATUS_PROCESS_INFO, Some(buffer), &mut needed);
            let _ = CloseServiceHandle(service);
            queried.ok()?;
            Some(status.dwProcessId).filter(|&pid| pid != 0)
        }
    }
}
