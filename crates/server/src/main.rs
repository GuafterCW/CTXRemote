//! `ctxremote-server`: hands out device IDs, introduces viewers to hosts and
//! relays their (end-to-end encrypted) traffic.
//!
//! ```text
//! ctxremote-server [--listen 0.0.0.0:21300] [--data ./data]
//! ```

mod registry;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport};
use ctxremote_proto::rendezvous::{verify_challenge, ClientMsg, ServerError, ServerMsg, SessionId};
use ctxremote_proto::{DeviceId, DEFAULT_PORT, PROTOCOL_VERSION};
use futures::StreamExt;
use rand::RngCore;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::registry::Registry;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);
/// Clients ping every 15 s; three missed pings drop the device.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const CONNECTS_PER_MINUTE: u32 = 30;

struct Server {
    registry: Mutex<Registry>,
    online: Mutex<HashMap<DeviceId, mpsc::Sender<ServerMsg>>>,
    pending: Mutex<HashMap<SessionId, oneshot::Sender<Transport>>>,
    rate: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut listen: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut data = PathBuf::from("data");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().context("--listen braucht eine Adresse")?.parse()?,
            "--data" => data = args.next().context("--data braucht einen Pfad")?.into(),
            "-h" | "--help" => {
                println!("ctxremote-server [--listen 0.0.0.0:{DEFAULT_PORT}] [--data ./data]");
                return Ok(());
            }
            other => bail!("unbekanntes Argument: {other}"),
        }
    }

    let server = Arc::new(Server {
        registry: Mutex::new(Registry::open(data.join("devices.json"))?),
        online: Mutex::default(),
        pending: Mutex::default(),
        rate: Mutex::default(),
    });

    let listener = TcpListener::bind(listen).await?;
    info!("CTXRemote-Server lauscht auf {listen}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let server = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server.handle(stream, peer).await {
                debug!(%peer, "Verbindung beendet: {e:#}");
            }
        });
    }
}

impl Server {
    async fn handle(self: Arc<Self>, stream: TcpStream, peer: SocketAddr) -> Result<()> {
        let mut t = framing::transport(stream);
        let mut nonce = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce);
        framing::send(&mut t, &ServerMsg::Challenge { version: PROTOCOL_VERSION, nonce }).await?;

        match timeout(HELLO_TIMEOUT, framing::recv::<ClientMsg>(&mut t)).await?? {
            ClientMsg::Register { id, public_key, signature } => {
                if !verify_challenge(&public_key, &nonce, &signature) {
                    framing::send(&mut t, &ServerMsg::Error(ServerError::BadSignature)).await?;
                    bail!("ungültige Signatur");
                }
                let id = self.registry.lock().unwrap().assign(id, public_key)?;
                framing::send(&mut t, &ServerMsg::Registered { id }).await?;
                self.control_channel(t, id, peer).await
            }
            ClientMsg::Connect { target } => {
                if !self.allow_connect(peer.ip()) {
                    framing::send(&mut t, &ServerMsg::Error(ServerError::RateLimited)).await?;
                    return Ok(());
                }
                self.connect(t, target).await
            }
            ClientMsg::Join { session } => {
                let waiting = self.pending.lock().unwrap().remove(&session);
                if let Some(viewer) = waiting {
                    let _ = viewer.send(t);
                }
                Ok(())
            }
            ClientMsg::Ping => Ok(()),
        }
    }

    /// Keeps a registered device reachable until it disconnects or goes silent.
    async fn control_channel(&self, mut t: Transport, id: DeviceId, peer: SocketAddr) -> Result<()> {
        let (tx, mut rx) = mpsc::channel(16);
        // A newer connection for the same ID replaces the old one, whose loop ends below.
        self.online.lock().unwrap().insert(id, tx.clone());
        info!(%id, %peer, "Gerät online");

        let result = async {
            loop {
                tokio::select! {
                    msg = rx.recv() => match msg {
                        Some(msg) => framing::send(&mut t, &msg).await?,
                        None => return Ok(()),
                    },
                    frame = timeout(IDLE_TIMEOUT, t.next()) => match frame? {
                        Some(frame) => match framing::decode(&frame?)? {
                            ClientMsg::Ping => framing::send(&mut t, &ServerMsg::Pong).await?,
                            other => bail!("unerwartete Nachricht: {other:?}"),
                        },
                        None => return Ok(()),
                    },
                }
            }
        }
        .await;

        let mut online = self.online.lock().unwrap();
        if online.get(&id).is_some_and(|current| current.same_channel(&tx)) {
            online.remove(&id);
            info!(%id, "Gerät offline");
        }
        result
    }

    async fn connect(&self, mut viewer: Transport, target: DeviceId) -> Result<()> {
        let Some(host) = self.online.lock().unwrap().get(&target).cloned() else {
            framing::send(&mut viewer, &ServerMsg::Error(ServerError::Offline)).await?;
            return Ok(());
        };

        let mut session = SessionId::default();
        rand::thread_rng().fill_bytes(&mut session);
        let (join_tx, join_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(session, join_tx);

        if host.send(ServerMsg::Incoming { session }).await.is_err() {
            self.pending.lock().unwrap().remove(&session);
            framing::send(&mut viewer, &ServerMsg::Error(ServerError::Offline)).await?;
            return Ok(());
        }

        let mut host = match timeout(JOIN_TIMEOUT, join_rx).await {
            Ok(Ok(host)) => host,
            _ => {
                self.pending.lock().unwrap().remove(&session);
                framing::send(&mut viewer, &ServerMsg::Error(ServerError::Timeout)).await?;
                return Ok(());
            }
        };

        framing::send(&mut viewer, &ServerMsg::Ready).await?;
        framing::send(&mut host, &ServerMsg::Ready).await?;
        info!(%target, "Sitzung wird weitergeleitet");
        let (up, down) = relay(viewer, host).await?;
        info!(%target, up, down, "Sitzung beendet");
        Ok(())
    }

    fn allow_connect(&self, ip: IpAddr) -> bool {
        let mut rate = self.rate.lock().unwrap();
        let now = Instant::now();
        let entry = rate.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) > Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > CONNECTS_PER_MINUTE {
            warn!(%ip, "Verbindungsversuche gedrosselt");
            return false;
        }
        true
    }
}

/// Pipes bytes between both peers until either side closes.
async fn relay(a: Transport, b: Transport) -> Result<(u64, u64)> {
    let a = a.into_parts();
    let b = b.into_parts();
    let (mut a_io, mut b_io) = (a.io, b.io);
    // Anything already read past the last frame belongs to the other side.
    if !a.read_buf.is_empty() {
        b_io.write_all(&a.read_buf).await?;
    }
    if !b.read_buf.is_empty() {
        a_io.write_all(&b.read_buf).await?;
    }
    Ok(tokio::io::copy_bidirectional(&mut a_io, &mut b_io).await?)
}
