//! The controlled side: stays reachable at the server and serves sessions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::{sign_challenge, ClientMsg, ServerMsg, SessionId};
use ctxremote_proto::secure::{self, host_handshake, Refusal, SecureReceiver, SecureSender};
use ctxremote_proto::session::{HostInfo, HostMsg, VideoCodec, VideoFrame, ViewerMsg};
use ctxremote_proto::DeviceId;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use crate::capture::{self, Capturer};
use crate::config::{generate_password, Config};
use crate::encoder::{pack_bgra, VideoEncoder};
use crate::input::Injector;
use crate::net;

const PING_INTERVAL: Duration = Duration::from_secs(15);
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(5 * 60);
const FPS: f32 = 30.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Presence {
    Connecting,
    Online { id: String },
    Offline { reason: String },
}

#[derive(Debug, Clone, Serialize)]
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
}

#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    /// Starts the presence loop on the current tokio runtime.
    pub fn start(config: Arc<RwLock<Config>>) -> Self {
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
        });
        tokio::spawn(presence_loop(shared.clone()));
        Self { shared }
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

    let peer = match timeout(Duration::from_secs(10), rx.recv::<ViewerMsg>()).await?? {
        Some(ViewerMsg::Hello { name, .. }) => name,
        _ => bail!("Gegenstelle hat sich nicht vorgestellt"),
    };

    let number = shared.next_session.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(Notify::new());
    shared.sessions.lock().unwrap().insert(number, (peer.clone(), stop.clone()));
    let _ = shared.events.send(HostEvent::SessionStarted { session: number, peer: peer.clone() });
    info!(%peer, "Sitzung gestartet");

    let result = run_session(tx, rx, stop).await;

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

enum VideoCommand {
    Keyframe,
    Display(u8),
}

async fn run_session(mut tx: SecureSender, mut rx: SecureReceiver, stop: Arc<Notify>) -> Result<()> {
    let displays = match capture::displays() {
        Ok(displays) if !displays.is_empty() => displays,
        result => {
            let reason = result.err().map_or("Kein Bildschirm gefunden".into(), |e| format!("{e:#}"));
            // Tell the viewer why instead of just dropping the connection.
            let _ = tx.send(&HostMsg::Bye(reason.clone())).await;
            bail!(reason);
        }
    };
    let primary = displays.iter().find(|d| d.primary).unwrap_or(&displays[0]);
    let mut active = primary.clone();
    tx.send(&HostMsg::Welcome(HostInfo {
        hostname: whoami::devicename(),
        username: whoami::username(),
        os: format!("{}", whoami::distro()),
        displays: displays.iter().map(Into::into).collect(),
        active_display: active.index,
    }))
    .await?;

    // Capture and encoding block, so they live on their own thread. The small
    // channel applies backpressure: on a slow link we capture less often
    // instead of queueing stale frames.
    let (frames_tx, mut frames_rx) = mpsc::channel::<VideoFrame>(2);
    let (commands, commands_rx) = std_mpsc::channel::<VideoCommand>();
    let first_display = active.index;
    let video = std::thread::Builder::new()
        .name("ctxremote-video".into())
        .spawn(move || {
            if let Err(e) = video_loop(first_display, commands_rx, frames_tx) {
                warn!("Videoübertragung beendet: {e:#}");
            }
        })?;

    let mut injector = Injector::new(&active);
    let result: Result<()> = async {
        loop {
            tokio::select! {
                frame = frames_rx.recv() => match frame {
                    Some(frame) => tx.send(&HostMsg::Video(frame)).await?,
                    None => bail!("Bildschirmaufnahme nicht verfügbar"),
                },
                msg = rx.recv::<ViewerMsg>() => match msg? {
                    Some(ViewerMsg::Input(event)) => injector.apply(&event),
                    Some(ViewerMsg::RequestKeyframe) => { let _ = commands.send(VideoCommand::Keyframe); }
                    Some(ViewerMsg::SelectDisplay(index)) => {
                        if let Some(display) = displays.iter().find(|d| d.index == index) {
                            active = display.clone();
                            injector.set_display(&active);
                            let _ = commands.send(VideoCommand::Display(index));
                        }
                    }
                    Some(ViewerMsg::Clipboard(_)) | Some(ViewerMsg::Hello { .. }) => {}
                    Some(ViewerMsg::Bye) | None => return Ok(()),
                },
                _ = stop.notified() => {
                    let _ = tx.send(&HostMsg::Bye("Die Sitzung wurde am Gerät beendet".into())).await;
                    return Ok(());
                }
            }
        }
    }
    .await;

    injector.release_all();
    drop(commands);
    drop(frames_rx);
    let _ = tokio::task::spawn_blocking(move || video.join()).await;
    tx.close().await;
    result
}

fn video_loop(
    mut display: u8,
    commands: std_mpsc::Receiver<VideoCommand>,
    frames: mpsc::Sender<VideoFrame>,
) -> Result<()> {
    let frame_time = Duration::from_secs_f32(1.0 / FPS);
    let started = Instant::now();
    'display: loop {
        let mut capturer = Capturer::new(display)?;
        let mut encoder: Option<VideoEncoder> = None;
        let mut packed = Vec::new();
        let mut have_frame = false;
        let mut force_key = true;
        loop {
            let tick = Instant::now();
            loop {
                match commands.try_recv() {
                    Ok(VideoCommand::Keyframe) => force_key = true,
                    Ok(VideoCommand::Display(index)) if index != display => {
                        display = index;
                        continue 'display;
                    }
                    Ok(VideoCommand::Display(_)) => {}
                    Err(std_mpsc::TryRecvError::Empty) => break,
                    Err(std_mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }

            let changed = capturer.next_frame(frame_time.as_millis() as u32, |data, pitch, w, h| {
                pack_bgra(data, pitch, w, h, &mut packed);
            })?;
            have_frame |= changed;
            if !have_frame || !(changed || force_key) {
                continue;
            }

            let (width, height) = (capturer.display().width, capturer.display().height);
            if encoder.as_ref().map(|e| e.size()) != Some((width & !1, height & !1)) {
                encoder = Some(VideoEncoder::new(width, height, FPS)?);
            }
            let encoder = encoder.as_mut().expect("created above");
            if force_key {
                encoder.force_keyframe();
            }
            let (w, h) = encoder.size();
            let encoded = encoder.encode(&packed)?;
            if encoded.data.is_empty() {
                // Rate control skipped this frame; a pending keyframe is retried next tick.
                continue;
            }
            force_key &= !encoded.keyframe;
            let frame = VideoFrame {
                display,
                width: w,
                height: h,
                keyframe: encoded.keyframe,
                codec: VideoCodec::H264,
                timestamp_us: started.elapsed().as_micros() as u64,
                data: encoded.data.to_vec(),
            };
            if frames.blocking_send(frame).is_err() {
                return Ok(());
            }
            if let Some(rest) = frame_time.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}
