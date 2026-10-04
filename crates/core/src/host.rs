//! The controlled side: stays reachable at the server and serves sessions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::{sign_challenge, ClientMsg, ServerMsg, SessionId};
use ctxremote_proto::framing::Transport;
use ctxremote_proto::secure::{self, host_handshake, Refusal, SecureReceiver, SecureSender, TransportStream};
use ctxremote_proto::session::{Features, HostMsg, ViewerMsg};
use ctxremote_proto::DeviceId;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use crate::agent;
use crate::config::{generate_password, Config, DirectSettings};
use crate::direct::{DirectListener, Offer};
use crate::net;

const PING_INTERVAL: Duration = Duration::from_secs(15);
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(5 * 60);
/// Must stay below the viewer's wait for `Welcome`.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

/// Decides whether the named viewer may connect, e.g. by asking the user.
pub type Approver = Arc<dyn Fn(String) -> BoxFuture<'static, bool> + Send + Sync>;

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
    SessionStarted { session: u64, peer: String },
    SessionEnded { session: u64 },
    PasswordChanged,
}

struct Shared {
    config: Arc<RwLock<Config>>,
    password: Mutex<String>,
    failures: Mutex<(u32, Option<Instant>)>,
    sessions: Mutex<HashMap<u64, (String, Arc<Notify>)>>,
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
        });
        tokio::spawn(presence_loop(shared.clone()));
        let (enabled, port, extra) = {
            let config = shared.config.read().unwrap();
            (config.direct, config.direct_port, config.direct_addresses.clone())
        };
        if enabled {
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
        if let Some((_, stop)) = self.shared.sessions.lock().unwrap().get(&session) {
            stop.notify_one();
        }
    }

    /// Sessions currently controlling this device, with the viewer's name.
    pub fn sessions(&self) -> Vec<(u64, String)> {
        let sessions = self.shared.sessions.lock().unwrap();
        let mut list: Vec<_> = sessions.iter().map(|(n, (peer, _))| (*n, peer.clone())).collect();
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
    framing::send(&mut t, &register).await?;
    let id = match framing::recv::<ServerMsg>(&mut t).await? {
        ServerMsg::Registered { id } => id,
        ServerMsg::Error(e) => return Err(e.into()),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    if requested != Some(id) {
        let mut config = shared.config.write().unwrap();
        config.device_id = Some(id);
        config.save()?;
    }
    info!(%id, "beim Server angemeldet");
    shared.presence.send_replace(Presence::Online { id: id.to_string() });

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
    let permanent = shared.config.read().unwrap().permanent_password.clone();
    let mut passwords = vec![one_time.as_str()];
    passwords.extend(permanent.as_deref().filter(|p| !p.is_empty()));

    let (tx, mut rx, slot) = match host_handshake(t, id, &passwords).await {
        Ok(ok) => ok,
        Err(e) => {
            if e.downcast_ref::<Refusal>() == Some(&Refusal::WrongPassword) {
                record_failure(&shared);
            }
            return Err(e);
        }
    };
    *shared.failures.lock().unwrap() = (0, None);

    let (peer, features) = match timeout(Duration::from_secs(10), rx.recv_with_trailer::<ViewerMsg, Features>()).await?? {
        Some((ViewerMsg::Hello { name, .. }, features)) => (name, features.unwrap_or(Features::NONE)),
        _ => bail!("Gegenstelle hat sich nicht vorgestellt"),
    };

    let approver = shared.approver.lock().unwrap().clone();
    if let Some(approve) = approver {
        // Viewer messages sent meanwhile (input) wait unread in the connection.
        if !timeout(APPROVAL_TIMEOUT, approve(peer.clone())).await.unwrap_or(false) {
            let mut tx = tx;
            let _ = tx.send(&HostMsg::Bye("Der Zugriff wurde abgelehnt".into())).await;
            tx.close().await;
            bail!("Zugriff für {peer} abgelehnt");
        }
    }

    let number = shared.next_session.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(Notify::new());
    shared.sessions.lock().unwrap().insert(number, (peer.clone(), stop.clone()));
    let _ = shared.events.send(HostEvent::SessionStarted { session: number, peer: peer.clone() });
    info!(%peer, "Sitzung gestartet");

    let route = Route {
        features,
        direct: shared.direct.lock().unwrap().clone(),
        server: shared.config.read().unwrap().server_addr(),
    };
    let result = run_session(tx, rx, stop, shared.screen.as_ref(), route).await;

    shared.sessions.lock().unwrap().remove(&number);
    let _ = shared.events.send(HostEvent::SessionEnded { session: number });
    // A used one-time password is spent, as with any OTP.
    if slot == 0 {
        *shared.password.lock().unwrap() = generate_password();
        let _ = shared.events.send(HostEvent::PasswordChanged);
    }
    info!(%peer, "Sitzung beendet");
    result
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
    /// What the viewer understands; newer messages only go to viewers that do.
    features: Features,
    direct: Option<Arc<DirectListener>>,
    server: String,
}

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
    route: Route,
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
    let result: Result<()> = async {
        loop {
            tokio::select! {
                msg = from_agent.recv() => match msg {
                    Some(HostMsg::Cursor(_)) if !route.features.has(Features::CURSOR) => {}
                    Some(msg @ HostMsg::Welcome(_)) => {
                        // Older viewers ignore the trailer; newer ones learn what we support.
                        tx.send_with_trailer(&msg, &Features::CURRENT).await?;
                        if let (false, true, Some(listener)) = (offered, route.features.has(Features::DIRECT), &route.direct) {
                            offered = true;
                            let new = listener.offer(&route.server);
                            if !new.addrs.is_empty() {
                                tx.send(&HostMsg::DirectOffer { addrs: new.addrs.clone(), token: new.token }).await?;
                                offer = Some(new);
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
                        }
                        None => {
                            let _ = tx.send(&HostMsg::Bye("Der Bildschirm ist nicht mehr verfügbar".into())).await;
                            return Ok(());
                        }
                    },
                },
                msg = rx.recv::<ViewerMsg>() => match msg? {
                    Some(ViewerMsg::Bye) | None => return Ok(()),
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
