//! The controlling side of a session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport};
use ctxremote_proto::rendezvous::ClientMsg;
use ctxremote_proto::secure::{viewer_handshake, TransportSink, TransportStream};
use ctxremote_proto::session::{CursorShape, Features, HelloExtras, HelperProfile, HostInfo, HostMsg, VideoFrame, ViewerMsg};
use ctxremote_proto::DeviceId;
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tracing::{debug, info};

use crate::files::client::{FileClient, TransferEvent};
use crate::net;
use crate::punch::PunchViewer;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The host may first ask its user (see `Host::require_approval`).
const WELCOME_TIMEOUT: Duration = Duration::from_secs(90);
/// How long the viewer waits for its direct connection once the host switched.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(10);
const OUTDATED: &str = "Das ferngesteuerte Gerät braucht dafür eine neuere CTXRemote-Version";

/// Hands the first confirmed direct connection to the session; later ones are dropped.
#[derive(Clone)]
struct Found(Arc<Mutex<Option<(oneshot::Sender<(TransportStream, String)>, mpsc::UnboundedSender<TransportSink>)>>>);

impl Found {
    fn new(found: oneshot::Sender<(TransportStream, String)>, reroute: mpsc::UnboundedSender<TransportSink>) -> Self {
        Self(Arc::new(Mutex::new(Some((found, reroute)))))
    }

    fn deliver(&self, t: Transport, addr: String) {
        let Some((found, reroute)) = self.0.lock().unwrap().take() else { return };
        let (sink, stream) = t.split();
        let _ = found.send((stream, addr));
        let _ = reroute.send(sink);
    }
}

pub enum ViewerEvent {
    Video(VideoFrame),
    Clipboard(String),
    /// The host's pointer changed shape.
    Cursor(CursorShape),
    /// Progress or outcome of a file transfer started through [`ViewerSession::files`].
    Transfer(TransferEvent),
    /// The session now runs over a direct connection to this address.
    Direct(String),
    /// A chat message from the person at the host.
    Chat(String),
    /// The session ended; carries the reason if it was not the viewer's choice.
    Closed(Option<String>),
}

pub struct ViewerSession {
    pub host: HostInfo,
    /// What the host understands; older hosts report none.
    pub features: Features,
    outbox: mpsc::UnboundedSender<ViewerMsg>,
    files: Arc<FileClient>,
}

impl ViewerSession {
    /// Connects through `server` and authenticates with `password`.
    /// `on_event` runs on the network task and must not block.
    pub async fn connect(
        server: &str,
        own_id: Option<DeviceId>,
        target: DeviceId,
        password: &str,
        profile: Option<HelperProfile>,
        on_event: impl Fn(ViewerEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let (mut t, _) = net::dial(server).await?;
        framing::send(&mut t, &ClientMsg::Connect { target }).await?;
        net::await_ready(&mut t, CONNECT_TIMEOUT).await?;

        let (mut tx, mut rx) = viewer_handshake(t, target, password).await?;
        let name = format!("{} ({})", whoami::username(), whoami::devicename());
        // Older hosts read only the features from the trailer, or nothing; newer ones answer with theirs on `Welcome`.
        let extras = HelloExtras { features: Features::CURRENT, profile };
        tx.send_with_trailer(&ViewerMsg::Hello { name, device: own_id }, &extras).await?;
        let (host, features) = match timeout(WELCOME_TIMEOUT, rx.recv_with_trailer::<HostMsg, Features>()).await?? {
            Some((HostMsg::Welcome(info), features)) => (info, features.unwrap_or(Features::NONE)),
            Some((HostMsg::Bye(reason), _)) => bail!(reason),
            _ => bail!("Gegenstelle hat die Sitzung nicht eröffnet"),
        };

        let on_event = Arc::new(on_event);
        let (outbox, mut outgoing) = mpsc::unbounded_channel::<ViewerMsg>();
        let files = FileClient::new(outbox.clone(), {
            let on_event = on_event.clone();
            Arc::new(move |event| on_event(ViewerEvent::Transfer(event)))
        });
        // The receiving task hands over the direct connection's writing half here.
        let (reroute, mut rerouted) = mpsc::unbounded_channel::<TransportSink>();
        let gate = files.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = outgoing.recv() => {
                        let Some(msg) = msg else { break };
                        // A message the host does not know would end its session.
                        let supported = match &msg {
                            ViewerMsg::File { .. } | ViewerMsg::Transfer { .. } => features.has(Features::FILES),
                            ViewerMsg::Restart => features.has(Features::RESTART),
                            ViewerMsg::SetQuality(_) => features.has(Features::QUALITY),
                            ViewerMsg::Chat(_) => features.has(Features::CHAT),
                            ViewerMsg::PunchAnswer { .. } => features.has(Features::PUNCH),
                            _ => true,
                        };
                        if !supported {
                            if let ViewerMsg::File { req, .. } = msg {
                                gate.reply(req, Err(OUTDATED.into()));
                            }
                            continue;
                        }
                        let bye = matches!(msg, ViewerMsg::Bye);
                        if tx.send(&msg).await.is_err() || bye {
                            break;
                        }
                    }
                    Some(sink) = rerouted.recv() => {
                        // `Switch` is the last message on the relay.
                        if tx.send(&ViewerMsg::Switch).await.is_err() {
                            break;
                        }
                        let mut relay = tx.reroute(sink);
                        let _ = futures::SinkExt::close(&mut relay).await;
                    }
                }
            }
            tx.close().await;
        });

        let router = files.clone();
        // Weak, so this task does not keep the sending side alive.
        let (server, answers) = (server.to_string(), outbox.downgrade());
        tokio::spawn(async move {
            let mut direct: Option<oneshot::Receiver<(TransportStream, String)>> = None;
            // Whichever route (TCP or punched UDP) confirms first takes the session.
            let mut found: Option<Found> = None;
            let mut token = None;
            let reason = loop {
                match rx.recv::<HostMsg>().await {
                    Ok(Some(HostMsg::DirectOffer { addrs, token: offered })) => {
                        if direct.is_some() {
                            continue;
                        }
                        let (tx, wait) = oneshot::channel();
                        direct = Some(wait);
                        token = Some(offered);
                        let deliver = Found::new(tx, reroute.clone());
                        found = Some(deliver.clone());
                        // `CTXREMOTE_DIRECT=udp` skips TCP, so the path through NAT can be
                        // tried inside a LAN (and tested).
                        if std::env::var("CTXREMOTE_DIRECT").is_ok_and(|v| v == "udp") {
                            continue;
                        }
                        tokio::spawn(async move {
                            match crate::direct::dial(&addrs, offered).await {
                                Ok((t, addr)) => deliver.deliver(t, addr),
                                Err(e) => debug!("Direktverbindung nicht möglich, bleibe beim Server: {e:#}"),
                            }
                        });
                    }
                    // A UDP route through NAT for the same token: tell the host our
                    // addresses, then both sides punch towards each other.
                    Ok(Some(HostMsg::PunchOffer { candidates, cert })) => {
                        let (Some(deliver), Some(token)) = (found.take(), token) else { continue };
                        let (server, answers) = (server.clone(), answers.clone());
                        tokio::spawn(async move {
                            let result = async {
                                let punch = PunchViewer::prepare(&server).await?;
                                answers
                                    .upgrade()
                                    .context("Sitzung beendet")?
                                    .send(ViewerMsg::PunchAnswer { candidates: punch.candidates().to_vec() })?;
                                punch.dial(&candidates, cert, token).await
                            };
                            match result.await {
                                Ok((t, addr)) => deliver.deliver(t, addr),
                                Err(e) => debug!("Direktverbindung durch NAT nicht möglich: {e:#}"),
                            }
                        });
                    }
                    // The host's answer to our `Switch` and its last message on the
                    // relay; the rest comes directly. The connection was confirmed
                    // before we switched, so its reading half is waiting already.
                    Ok(Some(HostMsg::Switch)) => {
                        let Some(wait) = direct.take() else {
                            break Some("Unerwarteter Verbindungswechsel".into());
                        };
                        match timeout(SWITCH_TIMEOUT, wait).await {
                            Ok(Ok((stream, addr))) => {
                                drop(rx.reroute(stream));
                                info!(%addr, "Sitzung läuft direkt");
                                on_event(ViewerEvent::Direct(addr));
                            }
                            _ => break Some("Wechsel auf die Direktverbindung fehlgeschlagen".into()),
                        }
                    }
                    Ok(Some(HostMsg::Video(frame))) => on_event(ViewerEvent::Video(frame)),
                    Ok(Some(HostMsg::Clipboard(text))) => on_event(ViewerEvent::Clipboard(text)),
                    Ok(Some(HostMsg::Bye(reason))) => break Some(reason),
                    Ok(Some(HostMsg::Cursor(shape))) => on_event(ViewerEvent::Cursor(shape)),
                    Ok(Some(HostMsg::Chat(text))) => {
                        if let Some(text) = crate::host::chat_text(&text) {
                            on_event(ViewerEvent::Chat(text));
                        }
                    }
                    Ok(Some(HostMsg::FileReply { req, result })) => router.reply(req, result),
                    Ok(Some(HostMsg::Transfer { id, msg })) => router.transfer(id, msg),
                    Ok(Some(HostMsg::TransferAck { id, bytes })) => router.ack(id, bytes),
                    Ok(Some(HostMsg::Welcome(_))) => {}
                    Ok(None) => break None,
                    Err(e) => break Some(format!("Verbindung unterbrochen: {e}")),
                }
            };
            router.closed();
            on_event(ViewerEvent::Closed(reason));
        });

        Ok(Self { host, features, outbox, files })
    }

    pub fn send(&self, msg: ViewerMsg) {
        let _ = self.outbox.send(msg);
    }

    /// A handle for sending from other threads, e.g. the clipboard watcher.
    pub fn sender(&self) -> mpsc::UnboundedSender<ViewerMsg> {
        self.outbox.clone()
    }

    /// The host's file browser and transfers in both directions.
    pub fn files(&self) -> &Arc<FileClient> {
        &self.files
    }

    pub fn close(&self) {
        self.send(ViewerMsg::Bye);
    }
}

impl Drop for ViewerSession {
    fn drop(&mut self) {
        self.close();
    }
}
