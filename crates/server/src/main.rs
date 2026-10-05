//! `ctxremote-server`: hands out device IDs, introduces viewers to hosts and
//! relays their (end-to-end encrypted) traffic.
//!
//! ```text
//! ctxremote-server [--listen 0.0.0.0:21300] [--data ./data]
//! ctxremote-server update-keygen
//! ctxremote-server update-sign --platform P --version V --file F --out DIR [--notes TEXT]
//! ```
//!
//! Client releases placed in `<data>/updates` (by `update-sign`) are handed
//! out to clients that ask; see `docs/DEPLOY.md`.

mod registry;
mod updates;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport};
use ctxremote_proto::rendezvous::{
    verify_alias_claim, verify_challenge, AliasError, ClientMsg, ServerError, ServerMsg, SessionId,
};
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
/// Installer downloads served at once; each holds a file open.
const PARALLEL_DOWNLOADS: usize = 8;

struct Server {
    registry: Mutex<Registry>,
    online: Mutex<HashMap<DeviceId, mpsc::Sender<ServerMsg>>>,
    pending: Mutex<HashMap<SessionId, oneshot::Sender<Transport>>>,
    rate: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    updates: updates::Store,
    downloads: tokio::sync::Semaphore,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("update-keygen") => return updates::keygen(),
        Some("update-sign") => return updates::sign(args.skip(1).collect()),
        _ => {}
    }
    let mut listen: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut data = PathBuf::from("data");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().context("--listen braucht eine Adresse")?.parse()?,
            "--data" => data = args.next().context("--data braucht einen Pfad")?.into(),
            "-h" | "--help" => {
                println!("ctxremote-server [--listen 0.0.0.0:{DEFAULT_PORT}] [--data ./data]");
                println!("ctxremote-server update-keygen");
                println!("ctxremote-server update-sign --platform P --version V --file F --out DIR [--notes TEXT]");
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
        updates: updates::Store::new(data.join("updates")),
        downloads: tokio::sync::Semaphore::new(PARALLEL_DOWNLOADS),
    });

    let listener = TcpListener::bind(listen).await?;
    info!("CTXRemote-Server lauscht auf {listen}");
    // Without UDP, sessions still work; direct paths through NAT do not.
    match tokio::net::UdpSocket::bind(listen).await {
        Ok(socket) => {
            tokio::spawn(reflect(socket));
        }
        Err(e) => warn!("UDP-Reflektor nicht verfügbar: {e}"),
    }
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

/// Answers every client with the address its UDP packets come from.
async fn reflect(socket: tokio::net::UdpSocket) {
    let mut buf = [0u8; 512];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, from)) => {
                if let Some(reply) = ctxremote_proto::reflect::answer(&buf[..len], from) {
                    let _ = socket.send_to(&reply, from).await;
                }
            }
            Err(e) => debug!("UDP-Reflektor: {e}"),
        }
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
            ClientMsg::ClaimAlias { id, public_key, signature, alias } => {
                if !self.allow_connect(peer.ip()) {
                    framing::send(&mut t, &ServerMsg::Error(ServerError::RateLimited)).await?;
                    return Ok(());
                }
                let result = if verify_alias_claim(&public_key, &nonce, alias.as_deref(), &signature) {
                    self.registry.lock().unwrap().claim_alias(id, public_key, alias.as_deref())
                } else {
                    Err(AliasError::BadSignature)
                };
                match &result {
                    Ok(Some(alias)) => info!(%id, alias, "Alias vergeben"),
                    Ok(None) => info!(%id, "Alias entfernt"),
                    Err(e) => debug!(%id, "Alias abgelehnt: {e}"),
                }
                framing::send(&mut t, &ServerMsg::AliasClaimed(result)).await
            }
            // Rate-limited like connects, so aliases cannot be scanned quickly.
            ClientMsg::ResolveAlias { alias } => {
                if !self.allow_connect(peer.ip()) {
                    framing::send(&mut t, &ServerMsg::Error(ServerError::RateLimited)).await?;
                    return Ok(());
                }
                let id = self.registry.lock().unwrap().resolve_alias(&alias);
                framing::send(&mut t, &ServerMsg::AliasResolved(id)).await
            }
            ClientMsg::UpdateCheck { platform } => {
                let info = self.updates.latest(&platform);
                framing::send(&mut t, &ServerMsg::Update(info)).await
            }
            ClientMsg::UpdateDownload { platform, version } => {
                if !self.allow_connect(peer.ip()) {
                    framing::send(&mut t, &ServerMsg::Error(ServerError::RateLimited)).await?;
                    return Ok(());
                }
                let Ok(_slot) = self.downloads.try_acquire() else {
                    framing::send(&mut t, &ServerMsg::Error(ServerError::RateLimited)).await?;
                    return Ok(());
                };
                self.updates.send(&mut t, &platform, &version).await
            }
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
