//! The controlled side: stays reachable at the server and serves sessions.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::{sign_challenge, ClientMsg, HostCaps, ServerMsg, SessionId};
use ctxremote_proto::framing::Transport;
use ctxremote_proto::secure::{self, host_handshake, Refusal, SecureReceiver, SecureSender, TransportStream};
use ctxremote_proto::session::{Features, FileOp, HelloExtras, HostMsg, InputEvent, Permissions, Transfer, TunnelMsg, ViewerMsg, MAX_CHAT};
use ctxremote_proto::DeviceId;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::agent;
use crate::history::{History, Outcome, Visit};
use crate::config::{generate_password, Config, DirectSettings};
use crate::direct::{DirectListener, Offer};
use crate::profile::Profile;
use crate::punch::PunchHost;
use crate::net;

const PING_INTERVAL: Duration = Duration::from_secs(15);
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(5 * 60);
/// Must stay below the viewer's wait for `Welcome`.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the viewer may take to send the authenticator code; it asks the
/// user first, so like the approval this must stay below its wait for `Welcome`.
const CODE_TIMEOUT: Duration = Duration::from_secs(60);

/// Decides whether the named viewer may connect, e.g. by asking the user.
/// Also gets the profile the viewer sent, if any (self-declared).
pub type Approver = Arc<dyn Fn(String, Option<Profile>) -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Presence {
    Connecting,
    Online { id: String },
    Offline { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HostEvent {
    /// `chat`: the viewer can receive chat messages. `profile`: how the
    /// viewer presents itself (self-declared).
    SessionStarted {
        session: u64,
        peer: String,
        chat: bool,
        #[serde(default)]
        profile: Option<Profile>,
        /// What the viewer may do; the person at the host can change it.
        #[serde(default)]
        rights: Permissions,
    },
    SessionEnded { session: u64 },
    PasswordChanged,
    /// A chat message from the viewer of `session`.
    Chat { session: u64, text: String },
    /// The rights of `session` changed, or privacy mode went on or off.
    Rights { session: u64, rights: Permissions, privacy: bool },
    /// The viewer of `session` started or stopped recording it.
    Recording { session: u64, on: bool },
}

/// A running session as the rest of the host sees it.
struct SessionHandle {
    peer: String,
    profile: Option<Profile>,
    stop: Arc<Notify>,
    /// Chat messages to the viewer; `None` if the viewer has no chat.
    chat: Option<mpsc::UnboundedSender<String>>,
    rights: watch::Sender<Permissions>,
    /// Privacy mode is on (the host's screen is blank).
    privacy: Arc<AtomicBool>,
    /// The viewer records the session.
    recording: Arc<AtomicBool>,
}

struct Shared {
    config: Arc<RwLock<Config>>,
    password: Mutex<String>,
    failures: Mutex<(u32, Option<Instant>)>,
    sessions: Mutex<HashMap<u64, SessionHandle>>,
    next_session: AtomicU64,
    presence: watch::Sender<Presence>,
    events: broadcast::Sender<HostEvent>,
    reconnect: Notify,
    screen: Arc<dyn ScreenSource>,
    approver: Mutex<Option<Approver>>,
    /// Set once the listener for direct connections runs.
    direct: Mutex<Option<Arc<DirectListener>>>,
    /// Serializes listener changes.
    direct_turn: tokio::sync::Mutex<()>,
    /// Who connected when (see [`crate::history`]).
    history: History,
    /// The step of the last accepted authenticator code; each counts once.
    last_code_step: Mutex<Option<u64>>,
}

#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    /// Starts the presence loop on the current tokio runtime.
    pub fn start(config: Arc<RwLock<Config>>) -> Self {
        Self::start_with(config, Arc::new(InProcess))
    }

    /// Like [`Host::start`], with the screen side of each session served by `screen`.
    pub fn start_with(config: Arc<RwLock<Config>>, screen: Arc<dyn ScreenSource>) -> Self {
        let (presence, _) = watch::channel(Presence::Connecting);
        let (events, _) = broadcast::channel(32);
        let shared = Arc::new(Shared {
            config,
            password: Mutex::new(generate_password()),
            failures: Mutex::new((0, None)),
            sessions: Mutex::default(),
            next_session: AtomicU64::new(1),
            presence,
            events,
            reconnect: Notify::new(),
            screen,
            approver: Mutex::new(None),
            direct: Mutex::new(None),
            direct_turn: tokio::sync::Mutex::new(()),
            history: History::open(Config::path().ok().map(|p| History::path_for(&p))),
            last_code_step: Mutex::new(None),
        });
        tokio::spawn(presence_loop(shared.clone()));
        let (enabled, port, extra, listen) = {
            let config = shared.config.read().unwrap();
            (config.direct, config.direct_port, config.direct_addresses.clone(), config.direct_listen)
        };
        if enabled && !listen {
            *shared.direct.lock().unwrap() = Some(DirectListener::without_tcp());
        } else if enabled {
            tokio::spawn(restart_direct(shared.clone(), DirectSettings { enabled, port, addresses: extra }));
        }
        Self { shared }
    }

    /// Applies new direct-connection settings without a restart: stops the
    /// listener and starts a new one if enabled. Running sessions are unaffected.
    /// The caller updates the config. Needs a tokio runtime.
    pub fn set_direct(&self, settings: DirectSettings) {
        tokio::spawn(restart_direct(self.shared.clone(), settings));
    }

    /// Whether the listener for direct connections runs.
    pub fn direct_active(&self) -> bool {
        self.shared.direct.lock().unwrap().is_some()
    }

    /// The connection log, newest first.
    pub fn history(&self) -> Vec<Visit> {
        self.shared.history.visits()
    }

    pub fn presence(&self) -> watch::Receiver<Presence> {
        self.shared.presence.subscribe()
    }

    pub fn events(&self) -> broadcast::Receiver<HostEvent> {
        self.shared.events.subscribe()
    }

    pub fn password(&self) -> String {
        self.shared.password.lock().unwrap().clone()
    }

    pub fn refresh_password(&self) {
        *self.shared.password.lock().unwrap() = generate_password();
        let _ = self.shared.events.send(HostEvent::PasswordChanged);
    }

    /// Re-registers, e.g. after the server address changed.
    pub fn reconnect(&self) {
        self.shared.reconnect.notify_one();
    }

    /// Asks `approver` with the viewer's name before each session; a password alone
    /// is then not enough. Unanswered requests are refused after a minute.
    pub fn require_approval(&self, approver: Approver) {
        *self.shared.approver.lock().unwrap() = Some(approver);
    }

    pub fn end_session(&self, session: u64) {
        if let Some(handle) = self.shared.sessions.lock().unwrap().get(&session) {
            handle.stop.notify_one();
        }
    }

    /// Sends a chat message to the viewer of `session`.
    pub fn send_chat(&self, session: u64, text: &str) -> Result<(), String> {
        let text = chat_text(text).ok_or("Leere Nachricht")?;
        let sessions = self.shared.sessions.lock().unwrap();
        let handle = sessions.get(&session).ok_or("Die Sitzung ist beendet")?;
        let chat = handle.chat.as_ref().ok_or("Die Gegenstelle hat eine ältere Version ohne Chat")?;
        chat.send(text).map_err(|_| "Die Sitzung ist beendet".to_string())
    }

    /// The public alias as last confirmed by the server.
    pub fn public_alias(&self) -> Option<String> {
        self.shared.config.read().unwrap().public_alias.clone()
    }

    /// Sets (or with `None` drops) the public alias at the server and saves it.
    /// Returns it as stored (lowercase). Needs the device to be registered.
    pub async fn set_public_alias(&self, alias: Option<String>) -> Result<Option<String>, String> {
        let alias = alias.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
        if let Some(alias) = &alias {
            crate::alias::normalize_alias(alias).map_err(|e| e.to_string())?;
        }
        let (server, key, id) = {
            let config = self.shared.config.read().unwrap();
            let key = config.signing_key().map_err(|e| format!("{e:#}"))?;
            (config.server_addr(), key, config.device_id.ok_or("Das Gerät ist noch nicht am Server angemeldet")?)
        };
        let stored = crate::alias::claim(&server, &key, id, alias.as_deref())
            .await
            .map_err(|e| format!("{e:#}"))?;
        let mut config = self.shared.config.write().unwrap();
        config.public_alias = stored.clone();
        config.save().map_err(|e| format!("{e:#}"))?;
        Ok(stored)
    }

    /// Sessions whose viewer can receive chat messages.
    /// Sessions the viewer records.
    pub fn recording_sessions(&self) -> Vec<u64> {
        let sessions = self.shared.sessions.lock().unwrap();
        let mut list: Vec<u64> =
            sessions.iter().filter(|(_, h)| h.recording.load(Ordering::Relaxed)).map(|(n, _)| *n).collect();
        list.sort_unstable();
        list
    }

    pub fn chat_sessions(&self) -> Vec<u64> {
        let sessions = self.shared.sessions.lock().unwrap();
        let mut list: Vec<u64> = sessions.iter().filter(|(_, h)| h.chat.is_some()).map(|(n, _)| *n).collect();
        list.sort_unstable();
        list
    }

    /// Sessions currently controlling this device, with the viewer's name.
    /// The profiles of running sessions whose viewers sent one.
    pub fn session_profiles(&self) -> Vec<(u64, Profile)> {
        let sessions = self.shared.sessions.lock().unwrap();
        let mut list: Vec<_> = sessions.iter().filter_map(|(n, h)| Some((*n, h.profile.clone()?))).collect();
        list.sort_unstable_by_key(|(n, _)| *n);
        list
    }

    /// Changes what the viewer of `session` may do, from now on.
    pub fn set_rights(&self, session: u64, rights: Permissions) -> Result<(), String> {
        let sessions = self.shared.sessions.lock().unwrap();
        let handle = sessions.get(&session).ok_or("Die Sitzung ist beendet")?;
        handle.rights.send_replace(rights);
        Ok(())
    }

    /// Rights and privacy mode of each running session.
    pub fn session_rights(&self) -> Vec<(u64, Permissions, bool)> {
        let sessions = self.shared.sessions.lock().unwrap();
        let mut list: Vec<_> = sessions
            .iter()
            .map(|(n, h)| (*n, *h.rights.borrow(), h.privacy.load(Ordering::Relaxed)))
            .collect();
        list.sort_unstable_by_key(|(n, ..)| *n);
        list
    }

    pub fn sessions(&self) -> Vec<(u64, String)> {
        let sessions = self.shared.sessions.lock().unwrap();
        let mut list: Vec<_> = sessions.iter().map(|(n, h)| (*n, h.peer.clone())).collect();
        list.sort_unstable_by_key(|(n, _)| *n);
        list
    }
}

async fn restart_direct(shared: Arc<Shared>, settings: DirectSettings) {
    // One change at a time, so two quick saves cannot leave two listeners.
    let _turn = shared.direct_turn.lock().await;
    let old = shared.direct.lock().unwrap().take();
    let restarting = old.is_some();
    if let Some(old) = old {
        old.stop();
    }
    if !settings.enabled {
        return;
    }
    if !shared.config.read().unwrap().direct_listen {
        *shared.direct.lock().unwrap() = Some(DirectListener::without_tcp());
        return;
    }
    // The old sockets close once their aborted tasks are dropped; on a restart
    // the port may take a moment to come free, so try a few times.
    for attempt in 0..if restarting { 10 } else { 1 } {
        if attempt > 0 || restarting {
            sleep(Duration::from_millis(150)).await;
        }
        if let Some(listener) = DirectListener::start(settings.port, settings.addresses.clone()).await {
            *shared.direct.lock().unwrap() = Some(listener);
            return;
        }
    }
}

async fn presence_loop(shared: Arc<Shared>) {
    let mut backoff = Duration::from_secs(1);
    loop {
        shared.presence.send_replace(Presence::Connecting);
        let started = Instant::now();
        let reason = match stay_registered(&shared).await {
            Ok(()) => "Verbindung zum Server getrennt".to_string(),
            Err(e) => format!("{e:#}"),
        };
        warn!("Server nicht verbunden: {reason}");
        shared.presence.send_replace(Presence::Offline { reason });
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        tokio::select! {
            _ = sleep(backoff) => backoff = (backoff * 2).min(Duration::from_secs(30)),
            _ = shared.reconnect.notified() => backoff = Duration::from_secs(1),
        }
    }
}

async fn stay_registered(shared: &Arc<Shared>) -> Result<()> {
    let (server, requested, key) = {
        let config = shared.config.read().unwrap();
        (config.server_addr(), config.device_id, config.signing_key()?)
    };
    let (mut t, nonce) = net::dial(&server).await?;
    let register = ClientMsg::Register {
        id: requested,
        public_key: key.verifying_key().to_bytes(),
        signature: sign_challenge(&key, &nonce),
    };
    // The trailer tells newer servers what this host understands.
    framing::send_with_trailer(&mut t, &register, &HostCaps::CURRENT).await?;
    let id = match framing::recv::<ServerMsg>(&mut t).await? {
        ServerMsg::Registered { id } => id,
        ServerMsg::Error(e) => return Err(e.into()),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    if requested != Some(id) {
        if let Some(old) = requested {
            // Another key owns the old ID on the server.
            tracing::warn!(%old, %id, "Server hat die gespeicherte ID abgelehnt und eine neue vergeben");
        }
        let mut config = shared.config.write().unwrap();
        config.device_id = Some(id);
        config.save()?;
    }
    info!(%id, "beim Server angemeldet");
    shared.presence.send_replace(Presence::Online { id: id.to_string() });
    // The server may be new (or its data lost): claim the saved alias again.
    let alias = shared.config.read().unwrap().public_alias.clone();
    if let Some(alias) = alias {
        tokio::spawn(async move {
            match crate::alias::claim(&server, &key, id, Some(&alias)).await {
                Ok(_) => {}
                Err(e) => warn!(%alias, "Alias nicht bestätigt: {e:#}"),
            }
        });
    }

    let mut ping = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            _ = ping.tick() => framing::send(&mut t, &ClientMsg::Ping).await?,
            _ = shared.reconnect.notified() => return Ok(()),
            frame = t.next() => match framing::decode::<ServerMsg>(&frame.context("Server hat getrennt")??)? {
                ServerMsg::Incoming { session } => {
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        if let Err(e) = serve(shared, id, session).await {
                            info!("Sitzung abgebrochen: {e:#}");
                        }
                    });
                }
                ServerMsg::Pong => {}
                // Another device of the account wants a computer in this network awake.
                ServerMsg::Wake { macs } => {
                    tokio::task::spawn_blocking(move || match crate::wol::wake(&macs) {
                        Ok(_) => info!("Weckpaket für ein Gerät des Kontos gesendet"),
                        Err(e) => debug!("Wecken nicht möglich: {e:#}"),
                    });
                }
                ServerMsg::Error(e) => return Err(e.into()),
                other => bail!("unerwartete Serverantwort: {other:?}"),
            },
        }
    }
}

async fn serve(shared: Arc<Shared>, id: DeviceId, session: SessionId) -> Result<()> {
    let server = shared.config.read().unwrap().server_addr();
    let (mut t, _) = net::dial(&server).await?;
    framing::send(&mut t, &ClientMsg::Join { session }).await?;
    net::await_ready(&mut t, Duration::from_secs(20)).await?;

    if locked_out(&shared) {
        secure::refuse(&mut t, Refusal::LockedOut).await?;
        bail!("gesperrt nach zu vielen Fehlversuchen");
    }

    let one_time = shared.password.lock().unwrap().clone();
    let (permanent, grant) = {
        let config = shared.config.read().unwrap();
        (config.permanent_password.clone(), config.account_access.clone().filter(|g| g.host == id))
    };
    let mut passwords = vec![one_time.as_str()];
    passwords.extend(permanent.as_deref().filter(|p| !p.is_empty()));
    // Devices of the account: their own slot, see docs/ACCOUNTS.md.
    let account_slot = grant.as_ref().map(|g| {
        passwords.push(g.password.as_str());
        passwords.len() - 1
    });

    let (tx, mut rx, slot) = match host_handshake(t, id, &passwords).await {
        Ok(ok) => ok,
        Err(e) => {
            if e.downcast_ref::<Refusal>() == Some(&Refusal::WrongPassword) {
                record_failure(&shared);
                shared.history.add("", None, Outcome::WrongPassword);
            }
            return Err(e);
        }
    };
    *shared.failures.lock().unwrap() = (0, None);

    let (peer, extras) = match timeout(Duration::from_secs(10), rx.recv_with_rest::<ViewerMsg>()).await?? {
        Some((ViewerMsg::Hello { name, .. }, rest)) => (name, HelloExtras::decode(&rest)),
        _ => bail!("Gegenstelle hat sich nicht vorgestellt"),
    };
    let features = extras.features;
    let profile = extras.profile.and_then(Profile::from_wire);
    let profile_name = profile.as_ref().map(|p| p.name.clone()).filter(|n| !n.is_empty());

    // The account password alone is not enough: the viewer's device must
    // still be in the account (a removed device keeps the account key).
    if Some(slot) == account_slot {
        let grant = grant.as_ref().expect("account slot has a grant");
        let server = shared.config.read().unwrap().server_addr();
        let member = match (&extras.member, grant.witness_key()) {
            (Some(proof), Some(witness)) if proof.verify(id, &rx.binding()) => {
                crate::account::same_account(&server, proof.public_key, witness).await
            }
            _ => false,
        };
        if !member {
            let mut tx = tx;
            let _ = tx.send(&HostMsg::Bye("Dieses Gerät gehört nicht mehr zum Konto".into())).await;
            tx.close().await;
            record_failure(&shared);
            shared.history.add(&peer, profile_name.as_deref(), Outcome::NotMember);
            bail!("{peer}: Kontozugriff ohne gültige Mitgliedschaft");
        }
        info!(%peer, "Zugriff über das Konto");
    }

    // Two-factor: the permanent password also needs the authenticator code.
    let permanent_slot = slot != 0 && Some(slot) != account_slot;
    let code_secret = shared.config.read().unwrap().code_secret.clone();
    let mut tx = tx;
    if let (true, Some(secret)) = (permanent_slot, code_secret) {
        let refused = |reason: &str| reason.to_string();
        let problem = if !features.has(Features::CODE) {
            Some(refused("Dieses Gerät verlangt einen Bestätigungscode. Bitte CTXRemote aktualisieren."))
        } else {
            tx.send(&HostMsg::CodeRequired).await?;
            match timeout(CODE_TIMEOUT, rx.recv::<ViewerMsg>()).await {
                Ok(Ok(Some(ViewerMsg::Code(code)))) => {
                    let step = crate::totp::check(&secret, &code, crate::totp::now());
                    let mut last = shared.last_code_step.lock().unwrap();
                    match step {
                        // A code counts once; seen again it could be a replay.
                        Some(step) if *last < Some(step) => {
                            *last = Some(step);
                            None
                        }
                        Some(_) => Some(refused("Dieser Code wurde schon benutzt, bitte den nächsten abwarten")),
                        None => Some(refused("Der Bestätigungscode ist falsch")),
                    }
                }
                // The viewer gave up without a code.
                _ => {
                    tx.close().await;
                    bail!("{peer}: kein Bestätigungscode");
                }
            }
        };
        if let Some(reason) = problem {
            let _ = tx.send(&HostMsg::Bye(reason.clone())).await;
            tx.close().await;
            record_failure(&shared);
            shared.history.add(&peer, profile_name.as_deref(), Outcome::WrongCode);
            bail!("{peer}: {reason}");
        }
    }
    admitted(shared, slot, account_slot, tx, rx, peer, features, profile, profile_name).await
}

/// After the passwords (and the code): approval, then the session itself.
#[allow(clippy::too_many_arguments)]
async fn admitted(
    shared: Arc<Shared>,
    slot: usize,
    account_slot: Option<usize>,
    tx: SecureSender,
    rx: SecureReceiver,
    peer: String,
    features: Features,
    profile: Option<Profile>,
    profile_name: Option<String>,
) -> Result<()> {
    let approver = shared.approver.lock().unwrap().clone();
    if let Some(approve) = approver {
        // Viewer messages sent meanwhile (input) wait unread in the connection.
        if !timeout(APPROVAL_TIMEOUT, approve(peer.clone(), profile.clone())).await.unwrap_or(false) {
            let mut tx = tx;
            let _ = tx.send(&HostMsg::Bye("Der Zugriff wurde abgelehnt".into())).await;
            tx.close().await;
            shared.history.add(&peer, profile_name.as_deref(), Outcome::Declined);
            bail!("Zugriff für {peer} abgelehnt");
        }
    }

    let outcome = match slot {
        0 => Outcome::OneTimePassword,
        slot if Some(slot) == account_slot => Outcome::Account,
        _ => Outcome::PermanentPassword,
    };
    let visit = shared.history.add(&peer, profile_name.as_deref(), outcome);

    let number = shared.next_session.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(Notify::new());
    let (chat, chat_out) = mpsc::unbounded_channel::<String>();
    let chat = features.has(Features::CHAT).then_some(chat);
    let can_chat = chat.is_some();
    let initial = {
        let config = shared.config.read().unwrap();
        if slot == 0 { config.rights_attended } else { config.rights_unattended }
    };
    let (rights, rights_rx) = watch::channel(initial);
    let privacy = Arc::new(AtomicBool::new(false));
    let recording = Arc::new(AtomicBool::new(false));
    shared.sessions.lock().unwrap().insert(
        number,
        SessionHandle {
            peer: peer.clone(),
            profile: profile.clone(),
            stop: stop.clone(),
            chat,
            rights,
            privacy: privacy.clone(),
            recording: recording.clone(),
        },
    );
    let _ = shared.events.send(HostEvent::SessionStarted {
        session: number,
        peer: peer.clone(),
        chat: can_chat,
        profile,
        rights: initial,
    });
    info!(%peer, "Sitzung gestartet");

    let route = Route {
        number,
        events: shared.events.clone(),
        chat: chat_out,
        features,
        direct: shared.direct.lock().unwrap().clone(),
        server: shared.config.read().unwrap().server_addr(),
        rights: rights_rx,
        privacy,
        recording,
    };
    let result = run_session(tx, rx, stop, shared.screen.as_ref(), route).await;

    shared.sessions.lock().unwrap().remove(&number);
    shared.history.end(visit, &peer);
    let _ = shared.events.send(HostEvent::SessionEnded { session: number });
    // A used one-time password is spent, as with any OTP.
    if slot == 0 {
        *shared.password.lock().unwrap() = generate_password();
        let _ = shared.events.send(HostEvent::PasswordChanged);
    }
    info!(%peer, "Sitzung beendet");
    result
}

/// A chat message as it goes out or is shown: trimmed, not empty, at most
/// [`MAX_CHAT`] bytes (cut at a character boundary).
pub fn chat_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut end = text.len().min(MAX_CHAT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_string())
}

fn locked_out(shared: &Shared) -> bool {
    let mut failures = shared.failures.lock().unwrap();
    match failures.1 {
        Some(until) if Instant::now() < until => true,
        Some(_) => {
            *failures = (0, None);
            false
        }
        None => false,
    }
}

fn record_failure(shared: &Shared) {
    let mut failures = shared.failures.lock().unwrap();
    failures.0 += 1;
    if failures.0 >= MAX_FAILURES {
        warn!("zu viele falsche Passwörter, Zugang für {} Minuten gesperrt", LOCKOUT.as_secs() / 60);
        failures.1 = Some(Instant::now() + LOCKOUT);
    }
}

/// The two ends of a session's screen side: messages to it, messages from it.
/// It ends when the sender is dropped; it is gone when the receiver closes.
pub type ScreenChannels = (mpsc::Sender<ViewerMsg>, mpsc::Receiver<HostMsg>);

/// Provides the screen side (capture, input, clipboard) for each session.
pub trait ScreenSource: Send + Sync + 'static {
    fn open(&self) -> BoxFuture<'static, Result<ScreenChannels>>;
}

/// Runs [`agent::run`] as a task in this process.
pub struct InProcess;

impl ScreenSource for InProcess {
    fn open(&self) -> BoxFuture<'static, Result<ScreenChannels>> {
        let (to_agent, inbox) = mpsc::channel::<ViewerMsg>(64);
        // Small, so a slow link throttles capture instead of queueing frames.
        let (outbox, from_agent) = mpsc::channel::<HostMsg>(2);
        tokio::spawn(async move {
            if let Err(e) = agent::run(inbox, outbox).await {
                info!("Bildschirmseite beendet: {e:#}");
            }
        });
        Box::pin(async move { Ok((to_agent, from_agent)) })
    }
}

/// How long a vanished screen side may take to come back before the session ends.
const SCREEN_RETURN: Duration = Duration::from_secs(60);
/// How long the host waits for a direct connection the viewer says it opened;
/// it confirmed the connection already, so this is only the hand-over.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(3);

/// What a session needs to know about the viewer and the ways to reach it.
struct Route {
    /// The session's number, for chat events.
    number: u64,
    events: broadcast::Sender<HostEvent>,
    /// Chat messages typed at the host, to send to the viewer.
    chat: mpsc::UnboundedReceiver<String>,
    /// What the viewer understands; newer messages only go to viewers that do.
    features: Features,
    direct: Option<Arc<DirectListener>>,
    server: String,
    /// What the viewer may do, as set at the host.
    rights: watch::Receiver<Permissions>,
    privacy: Arc<AtomicBool>,
    recording: Arc<AtomicBool>,
}

impl Route {
    /// Reads the rights as they are now, not as last seen: a change must
    /// apply to the very next message, even before its notice is handled.
    fn allows(&self, right: u32) -> bool {
        self.rights.borrow().has(right)
    }

    fn rights_event(&self, rights: Permissions) {
        let privacy = self.privacy.load(Ordering::Relaxed);
        let _ = self.events.send(HostEvent::Rights { session: self.number, rights, privacy });
    }
}

/// The right a viewer message needs, if any.
fn needed_right(msg: &ViewerMsg) -> Option<u32> {
    match msg {
        // Drawing is a way of pointing, so it goes with mouse and keyboard.
        ViewerMsg::Input(_) | ViewerMsg::SecureAttention | ViewerMsg::LockScreen | ViewerMsg::Draw(_) => {
            Some(Permissions::INPUT)
        }
        // Puts files on the host's clipboard, so it needs both.
        ViewerMsg::File { op: FileOp::ClipboardFromDir { .. }, .. } => Some(Permissions::FILES | Permissions::CLIPBOARD),
        ViewerMsg::File { .. } | ViewerMsg::Transfer { .. } => Some(Permissions::FILES),
        ViewerMsg::Clipboard(_) | ViewerMsg::ClipboardImage(_) => Some(Permissions::CLIPBOARD),
        // Sound both ways goes with one right.
        ViewerMsg::SetAudio(true) | ViewerMsg::Mic(_) => Some(Permissions::AUDIO),
        ViewerMsg::Restart => Some(Permissions::RESTART),
        ViewerMsg::Privacy(true) => Some(Permissions::PRIVACY),
        ViewerMsg::Tunnel(TunnelMsg::Open { .. }) => Some(Permissions::TUNNEL),
        _ => None,
    }
}

const FILES_DENIED: &str = "Dateien sind in dieser Sitzung nicht erlaubt";

/// Moves the sending side onto the direct connection: `Switch` is the last
/// message on the relay. Returns the reading half.
async fn switch_sender(tx: &mut SecureSender, t: Transport) -> Result<TransportStream> {
    use futures::StreamExt as _;
    let (sink, stream) = t.split();
    tx.send(&HostMsg::Switch).await?;
    let mut relay = tx.reroute(sink);
    let _ = futures::SinkExt::close(&mut relay).await;
    Ok(stream)
}

/// Relays between the encrypted connection and the session's screen side.
async fn run_session(
    mut tx: SecureSender,
    mut rx: SecureReceiver,
    stop: Arc<Notify>,
    screen: &dyn ScreenSource,
    mut route: Route,
) -> Result<()> {
    let (mut to_agent, mut from_agent) = match screen.open().await {
        Ok(channels) => channels,
        Err(e) => {
            let _ = tx.send(&HostMsg::Bye(format!("{e:#}"))).await;
            tx.close().await;
            return Err(e);
        }
    };

    // The direct route on offer until the viewer switches to it.
    let mut offer: Option<Offer> = None;
    let mut offered = false;
    // The UDP side of the offer, for viewers behind NAT; prepared in the background.
    let (punch_ready, mut punch_prepared) = mpsc::unbounded_channel::<PunchHost>();
    let mut punch: Option<PunchHost> = None;
    let mut audio_on = false;
    let mut rights = *route.rights.borrow_and_update();
    // Transfers stopped because files were taken away; later parts are dropped.
    let mut stopped: HashSet<u32> = HashSet::new();
    // Port tunnels into this network; their traffic goes out with the rest.
    let (tunnel_out, mut tunnel_rx) = mpsc::unbounded_channel::<TunnelMsg>();
    let tunnels = crate::tunnel::TunnelHost::new(tunnel_out);
    let result: Result<()> = async {
        loop {
            tokio::select! {
                msg = from_agent.recv() => match msg {
                    Some(HostMsg::Cursor(_)) if !route.features.has(Features::CURSOR) => {}
                    Some(HostMsg::Audio(_)) if !route.features.has(Features::AUDIO) || !route.allows(Permissions::AUDIO) => {}
                    Some(HostMsg::Clipboard(_)) if !route.allows(Permissions::CLIPBOARD) => {}
                    Some(HostMsg::ClipboardImage(_))
                        if !route.features.has(Features::CLIPBOARD_IMAGE) || !route.allows(Permissions::CLIPBOARD) => {}
                    Some(HostMsg::ClipboardFiles(_))
                        if !route.features.has(Features::FILE_PASTE)
                            || !route.allows(Permissions::FILES | Permissions::CLIPBOARD) => {}
                    Some(HostMsg::Transfer { id, msg }) if !route.allows(Permissions::FILES) => {
                        // A download the host no longer allows: stop it on both ends.
                        if stopped.insert(id) && !matches!(msg, Transfer::End | Transfer::Failed(_) | Transfer::Cancel) {
                            let _ = to_agent.send(ViewerMsg::Transfer { id, msg: Transfer::Cancel }).await;
                            tx.send(&HostMsg::Transfer { id, msg: Transfer::Failed(FILES_DENIED.into()) }).await?;
                        }
                    }
                    Some(HostMsg::Privacy { on, error }) => {
                        route.privacy.store(on, Ordering::Relaxed);
                        route.rights_event(rights);
                        if route.features.has(Features::PRIVACY) {
                            tx.send(&HostMsg::Privacy { on, error }).await?;
                        }
                    }
                    Some(msg @ HostMsg::Welcome(_)) => {
                        // Older viewers ignore the trailer; newer ones learn what we support.
                        tx.send_with_trailer(&msg, &Features::CURRENT).await?;
                        if route.features.has(Features::RIGHTS) {
                            tx.send(&HostMsg::Rights(rights)).await?;
                        }
                        if let (false, true, Some(listener)) = (offered, route.features.has(Features::DIRECT), &route.direct) {
                            offered = true;
                            let new = listener.offer(&route.server);
                            // Without TCP addresses the offer still carries the token for the UDP path.
                            if !new.addrs.is_empty() || route.features.has(Features::PUNCH) {
                                tx.send(&HostMsg::DirectOffer { addrs: new.addrs.clone(), token: new.token }).await?;
                                offer = Some(new);
                                if route.features.has(Features::PUNCH) {
                                    let (server, ready) = (route.server.clone(), punch_ready.clone());
                                    tokio::spawn(async move {
                                        match PunchHost::prepare(&server).await {
                                            Ok(prepared) => { let _ = ready.send(prepared); }
                                            Err(e) => debug!("Kein Weg durch NAT: {e:#}"),
                                        }
                                    });
                                }
                            }
                        }
                    }
                    Some(msg) => {
                        let bye = matches!(msg, HostMsg::Bye(_));
                        tx.send(&msg).await?;
                        if bye {
                            return Ok(());
                        }
                    }
                    // Gone without `Bye`: the agent ended with its Windows session
                    // (sign-out, sign-in, user switch). Start one in the new session.
                    None => match reopen(screen, &stop).await {
                        Some((new_to, new_from)) => {
                            info!("Bildschirmseite neu gestartet");
                            (to_agent, from_agent) = (new_to, new_from);
                            // The new screen side starts silent.
                            if audio_on {
                                let _ = to_agent.send(ViewerMsg::SetAudio(true)).await;
                            }
                            // Likewise blank: e.g. after someone signed in at the computer.
                            if route.privacy.load(Ordering::Relaxed) {
                                let _ = to_agent.send(ViewerMsg::Privacy(true)).await;
                            }
                        }
                        None => {
                            let _ = tx.send(&HostMsg::Bye("Der Bildschirm ist nicht mehr verfügbar".into())).await;
                            return Ok(());
                        }
                    },
                },
                Some(text) = route.chat.recv() => tx.send(&HostMsg::Chat(text)).await?,
                Some(msg) = tunnel_rx.recv() => tx.send(&HostMsg::Tunnel(msg)).await?,
                Ok(()) = route.rights.changed() => {
                    rights = *route.rights.borrow_and_update();
                    info!(rights = rights.0, "Rechte der Sitzung geändert");
                    if route.features.has(Features::RIGHTS) {
                        tx.send(&HostMsg::Rights(rights)).await?;
                    }
                    if !rights.has(Permissions::INPUT) {
                        // No key or button may stay down at the host.
                        let _ = to_agent.send(ViewerMsg::Input(InputEvent::ReleaseAll)).await;
                    }
                    if audio_on && !rights.has(Permissions::AUDIO) {
                        audio_on = false;
                        let _ = to_agent.send(ViewerMsg::SetAudio(false)).await;
                    }
                    if !rights.has(Permissions::TUNNEL) {
                        tunnels.close_all();
                    }
                    if route.privacy.load(Ordering::Relaxed) && !rights.has(Permissions::PRIVACY) {
                        // The agent confirms with `Privacy { on: false }`.
                        let _ = to_agent.send(ViewerMsg::Privacy(false)).await;
                    }
                    route.rights_event(rights);
                }
                Some(prepared) = punch_prepared.recv() => {
                    // Only while the offer stands; after a switch it is moot.
                    if offer.is_some() {
                        let candidates = prepared.candidates().to_vec();
                        tx.send(&HostMsg::PunchOffer { candidates, cert: prepared.cert_hash() }).await?;
                        punch = Some(prepared);
                    }
                }
                msg = rx.recv::<ViewerMsg>() => match msg? {
                    Some(ViewerMsg::Bye) | None => return Ok(()),
                    // Cancelling a transfer is always allowed.
                    Some(msg @ ViewerMsg::Transfer { msg: Transfer::Cancel, .. }) => {
                        let _ = to_agent.send(msg).await;
                    }
                    Some(msg) if needed_right(&msg).is_some_and(|r| !route.allows(r)) => match msg {
                        // Answered, so the viewer does not wait for them.
                        ViewerMsg::File { req, .. } => {
                            tx.send(&HostMsg::FileReply { req, result: Err(FILES_DENIED.into()) }).await?;
                        }
                        ViewerMsg::Transfer { id, .. } => {
                            if stopped.insert(id) {
                                let _ = to_agent.send(ViewerMsg::Transfer { id, msg: Transfer::Cancel }).await;
                                tx.send(&HostMsg::Transfer { id, msg: Transfer::Cancel }).await?;
                            }
                        }
                        ViewerMsg::Tunnel(TunnelMsg::Open { id, .. }) => {
                            tunnels.refuse(id, "Port-Tunnel sind in dieser Sitzung nicht erlaubt");
                        }
                        ViewerMsg::Privacy(_) if route.features.has(Features::PRIVACY) => {
                            let error = Some("Der Privatsphäre-Modus ist in dieser Sitzung nicht erlaubt".to_string());
                            tx.send(&HostMsg::Privacy { on: false, error }).await?;
                        }
                        other => debug!("Nicht erlaubt in dieser Sitzung: {}", refused_kind(&other)),
                    },
                    Some(ViewerMsg::Tunnel(msg)) => tunnels.handle(msg),
                    // Only shown here; recording itself happens at the viewer.
                    Some(ViewerMsg::Recording(on)) => {
                        route.recording.store(on, Ordering::Relaxed);
                        info!(on, "Aufzeichnung der Sitzung");
                        let _ = route.events.send(HostEvent::Recording { session: route.number, on });
                    }
                    Some(ViewerMsg::Chat(text)) => {
                        if let Some(text) = chat_text(&text) {
                            let _ = route.events.send(HostEvent::Chat { session: route.number, text });
                        }
                    }
                    // The viewer's UDP addresses: punch towards them and accept its
                    // QUIC connection, which then takes the TCP route's token check.
                    Some(ViewerMsg::PunchAnswer { candidates }) => {
                        if let (Some(prepared), Some(listener), true) = (punch.take(), route.direct.clone(), offer.is_some()) {
                            tokio::spawn(async move {
                                let result = match prepared.accept(&candidates).await {
                                    Ok(t) => listener.admit_punched(t).await,
                                    Err(e) => Err(e),
                                };
                                if let Err(e) = result {
                                    debug!("Direktverbindung durch NAT nicht möglich: {e:#}");
                                }
                            });
                        }
                    }
                    // The viewer's last message on the relay. Only the viewer starts
                    // the switch, once it has our confirmation: had we switched on
                    // our own, a viewer that gave up waiting would lose the session.
                    // Its direct connection is with us by now, or a moment later.
                    Some(ViewerMsg::Switch) => {
                        let mut pending = offer.take().context("Wechsel ohne Angebot")?;
                        let t = timeout(SWITCH_TIMEOUT, &mut pending.connection)
                            .await
                            .context("Direktverbindung kam nicht an")?
                            .context("Direktverbindung kam nicht an")?;
                        let stream = switch_sender(&mut tx, t).await?;
                        drop(rx.reroute(stream));
                        info!("Sitzung läuft direkt");
                    }
                    // Windows accepts SendSAS only from the service process, which is this one.
                    Some(ViewerMsg::SecureAttention) => {
                        if let Err(e) = crate::sas::send() {
                            warn!("Strg+Alt+Entf fehlgeschlagen: {e:#}");
                        }
                    }
                    // Needs SeShutdownPrivilege, which the service has; the agent might not.
                    Some(ViewerMsg::Restart) => {
                        let reason = match crate::sas::restart() {
                            Ok(()) => "Das Gerät wird neu gestartet".to_string(),
                            Err(e) => format!("Neustart fehlgeschlagen: {e:#}"),
                        };
                        info!("{reason}");
                        let _ = tx.send(&HostMsg::Bye(reason)).await;
                        return Ok(());
                    }
                    Some(msg) => {
                        if let ViewerMsg::SetAudio(on) = msg {
                            audio_on = on;
                        }
                        // A failed send means the agent is gone; the branch above notices.
                        let _ = to_agent.send(msg).await;
                    }
                },
                _ = stop.notified() => {
                    let _ = tx.send(&HostMsg::Bye("Die Sitzung wurde am Gerät beendet".into())).await;
                    return Ok(());
                }
            }
        }
    }
    .await;

    // Tunnels hold their sockets until cut.
    tunnels.close_all();
    // Dropping the sender ends the screen side.
    drop(to_agent);
    drop(from_agent);
    tx.close().await;
    result
}

/// Retries opening the screen side for up to [`SCREEN_RETURN`]; `None` if it
/// stays away or the session is stopped meanwhile.
async fn reopen(screen: &dyn ScreenSource, stop: &Notify) -> Option<ScreenChannels> {
    let deadline = Instant::now() + SCREEN_RETURN;
    while Instant::now() < deadline {
        tokio::select! {
            _ = sleep(Duration::from_secs(1)) => {}
            _ = stop.notified() => return None,
        }
        match screen.open().await {
            Ok(channels) => return Some(channels),
            Err(e) => info!("Bildschirmseite noch nicht verfügbar: {e:#}"),
        }
    }
    None
}

/// What a refused message was, for the log: typed text, clipboard contents
/// and images stay out of it.
fn refused_kind(msg: &ViewerMsg) -> String {
    match msg {
        ViewerMsg::Input(ctxremote_proto::session::InputEvent::Text(_)) => "Input(Text)".into(),
        ViewerMsg::Clipboard(_) => "Clipboard".into(),
        ViewerMsg::ClipboardImage(_) => "ClipboardImage".into(),
        other => {
            let mut text = format!("{other:?}");
            text.truncate(text.floor_char_boundary(200));
            text
        }
    }
}

