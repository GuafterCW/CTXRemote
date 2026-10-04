//! Direct connections between viewer and host, bypassing the relay.
//!
//! Every session starts on the relay. A host with a listener then offers its
//! addresses and a one-time token inside the encrypted session; the viewer
//! tries them, presents the token in a plaintext [`DirectHello`], and both
//! sides move the running session over (see `docs/DIRECT.md`). The encryption
//! keys stay the same, so a direct connection is exactly as private as the
//! relayed one, and the server needs no changes. If nothing connects, the
//! session simply stays on the relay.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport, MAX_FRAME};
use ctxremote_proto::session::DirectHello;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Semaphore};
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Default TCP port of the direct listener, next to the server's 21300.
pub const DEFAULT_PORT: u16 = 21301;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);
/// Unauthenticated connections waiting for their hello at once.
const MAX_PENDING: usize = 16;

type Waiting = Mutex<HashMap<[u8; 32], oneshot::Sender<Transport>>>;

/// The host's listener for direct connections.
pub struct DirectListener {
    port: u16,
    waiting: Arc<Waiting>,
    /// Extra addresses offered verbatim, e.g. a forwarded port on the router.
    extra: Vec<String>,
}

/// A registered offer; dropping it withdraws the token.
pub struct Offer {
    pub addrs: Vec<String>,
    pub token: [u8; 32],
    /// Resolves with the connection once the viewer presented the token.
    pub connection: oneshot::Receiver<Transport>,
    waiting: Arc<Waiting>,
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.waiting.lock().unwrap().remove(&self.token);
    }
}

impl DirectListener {
    /// Listens on `port` for IPv4 and IPv6. `None` if neither can be bound
    /// (e.g. the port is taken); sessions then stay on the relay.
    pub async fn start(port: u16, extra: Vec<String>) -> Option<Arc<Self>> {
        let mut listeners = Vec::new();
        for addr in [SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)), SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))] {
            match bind(addr) {
                Ok(listener) => listeners.push(listener),
                Err(e) => debug!(%addr, "Direktverbindungen nicht möglich: {e:#}"),
            }
        }
        if listeners.is_empty() {
            warn!(port, "Direktverbindungen abgeschaltet: Port nicht verfügbar");
            return None;
        }
        info!(port, "Direktverbindungen möglich");
        let waiting: Arc<Waiting> = Arc::default();
        let gate = Arc::new(Semaphore::new(MAX_PENDING));
        for listener in listeners {
            tokio::spawn(accept_loop(listener, waiting.clone(), gate.clone()));
        }
        Some(Arc::new(Self { port, waiting, extra }))
    }

    /// Registers a fresh token and lists the addresses this host is likely
    /// reachable at, as seen on the way to `server`.
    pub fn offer(&self, server: &str) -> Offer {
        let token: [u8; 32] = rand::random();
        let (tx, connection) = oneshot::channel();
        self.waiting.lock().unwrap().insert(token, tx);
        let mut addrs: Vec<String> = local_ips(server)
            .into_iter()
            .map(|ip| SocketAddr::new(ip, self.port).to_string())
            .collect();
        addrs.extend(self.extra.iter().cloned());
        Offer { addrs, token, connection, waiting: self.waiting.clone() }
    }
}

/// Binds without the IPv4-mapped fallback on IPv6, so both sockets can share the port.
fn bind(addr: SocketAddr) -> Result<TcpListener> {
    use socket2::{Domain, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    // Lets the service restart without waiting for old connections to time out.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(64)?;
    Ok(TcpListener::from_std(socket.into())?)
}

async fn accept_loop(listener: TcpListener, waiting: Arc<Waiting>, gate: Arc<Semaphore>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                debug!("Direktverbindung nicht angenommen: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // Beyond the limit, new connections are dropped until a slot frees up.
        let Ok(permit) = gate.clone().try_acquire_owned() else { continue };
        let waiting = waiting.clone();
        tokio::spawn(async move {
            if let Err(e) = admit(stream, &waiting).await {
                debug!(%peer, "Direktverbindung abgelehnt: {e:#}");
            }
            drop(permit);
        });
    }
}

/// Reads the hello and hands the connection to the session that owns the token.
async fn admit(stream: TcpStream, waiting: &Waiting) -> Result<()> {
    let mut t = framing::transport(stream);
    // A stranger gets to send one small frame and nothing else.
    t.codec_mut().set_max_frame_length(256);
    let hello: DirectHello = timeout(HELLO_TIMEOUT, framing::recv(&mut t)).await.context("kein Hello")??;
    let session = waiting.lock().unwrap().remove(&hello.token).context("unbekanntes Token")?;
    t.codec_mut().set_max_frame_length(MAX_FRAME);
    // The echo tells the viewer it reached the right host before it moves over.
    framing::send(&mut t, &hello).await?;
    if session.send(t).is_err() {
        bail!("Sitzung schon beendet");
    }
    Ok(())
}

/// Viewer side: tries all addresses at once and returns the first connection
/// whose host confirmed the token.
pub async fn dial(addrs: &[String], token: [u8; 32]) -> Result<(Transport, String)> {
    let mut attempts: FuturesUnordered<_> = addrs.iter().take(8).map(|addr| attempt(addr, token)).collect();
    while let Some(result) = attempts.next().await {
        match result {
            Ok(connected) => return Ok(connected),
            Err(e) => debug!("Direktverbindung nicht möglich: {e:#}"),
        }
    }
    bail!("keine Adresse erreichbar")
}

async fn attempt(addr: &str, token: [u8; 32]) -> Result<(Transport, String)> {
    timeout(DIAL_TIMEOUT, async {
        let stream = TcpStream::connect(addr).await?;
        let mut t = framing::transport(stream);
        t.codec_mut().set_max_frame_length(256);
        framing::send(&mut t, &DirectHello { token }).await?;
        // Another device at the same address (e.g. the same LAN IP elsewhere) does not know the token.
        let echo: DirectHello = framing::recv(&mut t).await?;
        if echo.token != token {
            bail!("falsche Gegenstelle");
        }
        t.codec_mut().set_max_frame_length(MAX_FRAME);
        Ok((t, addr.to_string()))
    })
    .await
    .context("Zeitüberschreitung")?
}

/// The addresses of the interfaces that route towards `server` (IPv4) and
/// towards the public IPv6 internet. Asking the routing table needs no packets.
fn local_ips(server: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    let mut targets: Vec<SocketAddr> = server.to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    // Documentation addresses: never contacted, only used to pick a route.
    targets.push(SocketAddr::from(([192, 0, 2, 1], 9)));
    targets.push("[2001:db8::1]:9".parse().expect("valid"));
    let mut ips = Vec::new();
    for target in targets {
        let local: SocketAddr = if target.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { "[::]:0".parse().expect("valid") };
        let Ok(socket) = UdpSocket::bind(local) else { continue };
        if socket.connect(target).is_err() {
            continue;
        }
        let Ok(addr) = socket.local_addr() else { continue };
        let ip = addr.ip();
        if usable(ip) && !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    ips
}

fn usable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local(),
        // Link-local addresses would need a scope id the viewer cannot know.
        IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unspecified() && (ip.segments()[0] & 0xffc0) != 0xfe80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_admits_once() {
        // A free port; port 0 would give the IPv4 and IPv6 sockets different ones.
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let listener = DirectListener::start(port, vec![]).await.expect("listening");
        let mut offer = listener.offer("127.0.0.1:21300");
        let local = vec![format!("127.0.0.1:{port}")];

        let (mut viewer, addr) = dial(&local, offer.token).await.unwrap();
        assert_eq!(addr, local[0]);
        let mut host = timeout(Duration::from_secs(2), &mut offer.connection).await.unwrap().unwrap();
        framing::send(&mut viewer, &7u32).await.unwrap();
        assert_eq!(framing::recv::<u32>(&mut host).await.unwrap(), 7);

        // The token is spent; a second attempt finds no host.
        assert!(dial(&local, offer.token).await.is_err());
    }

    #[tokio::test]
    async fn withdrawn_offer_refuses() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let listener = DirectListener::start(port, vec!["example.org:1".into()]).await.unwrap();
        let offer = listener.offer("127.0.0.1:21300");
        assert!(offer.addrs.contains(&"example.org:1".to_string()));
        let token = offer.token;
        drop(offer);
        assert!(listener.waiting.lock().unwrap().is_empty());
        assert!(dial(&[format!("127.0.0.1:{port}")], token).await.is_err());
    }
}
