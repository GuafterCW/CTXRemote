//! Direct connections through NAT: UDP hole punching with QUIC on top.
//!
//! Both sides open a UDP socket and ask the server's reflector which public
//! address it has (see [`ctxremote_proto::reflect`]). They swap these
//! candidates inside the encrypted session, send packets towards each other
//! so their NATs let the other side's packets in, and the viewer then opens
//! a QUIC connection to the host. The host's certificate is pinned by the
//! hash it sent inside the session; on the QUIC stream both sides run the
//! same token check as on a TCP direct connection (see `docs/DIRECT.md`).
//!
//! This works with the usual home routers (endpoint-independent mapping), not
//! with symmetric NATs, e.g. some mobile carriers. Sessions then stay on the relay.

use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport, MAX_FRAME};
use ctxremote_proto::reflect;
use ctxremote_proto::session::DirectHello;
use futures::stream::{FuturesUnordered, StreamExt};
use quinn::{Connection, Endpoint, EndpointConfig, RecvStream, SendStream, TokioRuntime, TransportConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, Join, ReadBuf};
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};
use tracing::debug;

/// How long the reflector may take to answer, per try.
const REFLECT_WAIT: Duration = Duration::from_millis(700);
const REFLECT_TRIES: usize = 3;
/// From the swap of candidates until the connection must be confirmed.
const PUNCH_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE: &[u8] = b"ctxremote/punch";
const ALPN: &[u8] = b"ctxremote/1";
const SERVER_NAME: &str = "ctxremote";
/// Candidates tried at most, as with TCP.
const MAX_CANDIDATES: usize = 8;

/// A UDP socket and the addresses others can reach it at.
struct Gathered {
    socket: UdpSocket,
    candidates: Vec<String>,
}

/// Opens a UDP socket and learns its public address from `server`'s reflector.
/// Fails if the server does not answer (older servers have no reflector).
async fn gather(server: &str) -> Result<Gathered> {
    let target = tokio::net::lookup_host(server)
        .await?
        .find(SocketAddr::is_ipv4)
        .context("Server hat keine IPv4-Adresse")?;
    let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await?;
    let port = socket.local_addr()?.port();
    let id: [u8; 16] = rand::random();
    let mut public = None;
    let mut buf = [0u8; 128];
    'tries: for _ in 0..REFLECT_TRIES {
        socket.send_to(&reflect::request(id), target).await?;
        let deadline = tokio::time::Instant::now() + REFLECT_WAIT;
        while let Ok(received) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
            let Ok((len, from)) = received else { continue };
            if from == target {
                if let Some(addr) = reflect::parse_answer(&buf[..len], id) {
                    public = Some(addr);
                    break 'tries;
                }
            }
        }
    }
    let public = public.context("Der Server beantwortet keine UDP-Anfragen")?;
    let mut candidates = vec![public.to_string()];
    for ip in crate::direct::local_ips(server) {
        let local = SocketAddr::new(ip, port).to_string();
        if ip.is_ipv4() && !candidates.contains(&local) {
            candidates.push(local);
        }
    }
    Ok(Gathered { socket, candidates })
}

/// Usable IPv4 addresses out of the peer's candidates.
fn parse_candidates(candidates: &[String]) -> Vec<SocketAddr> {
    candidates
        .iter()
        .filter_map(|c| c.parse::<SocketAddr>().ok())
        .filter(|a| a.is_ipv4() && a.port() != 0 && !matches!(a.ip(), IpAddr::V4(ip) if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast()))
        .take(MAX_CANDIDATES)
        .collect()
}

/// Sends a few packets to each address, so this side's NAT expects answers from there.
async fn probe(socket: &UdpSocket, peers: &[SocketAddr]) {
    for round in 0..3 {
        if round > 0 {
            sleep(Duration::from_millis(60)).await;
        }
        for peer in peers {
            let _ = socket.send_to(PROBE, peer).await;
        }
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport_config() -> Arc<TransportConfig> {
    let mut config = TransportConfig::default();
    config
        .max_concurrent_bidi_streams(1u8.into())
        .max_concurrent_uni_streams(0u8.into())
        // Retries the first packets quickly while the other side's NAT opens.
        .initial_rtt(Duration::from_millis(100))
        // Keeps the NAT mappings open while the screen does not change.
        .keep_alive_interval(Some(Duration::from_secs(5)))
        .max_idle_timeout(Some(Duration::from_secs(30).try_into().expect("valid")))
        // Room for keyframes at high resolutions on links with some delay.
        .stream_receive_window((8u32 * 1024 * 1024).into())
        .receive_window((16u32 * 1024 * 1024).into())
        .send_window(16 * 1024 * 1024);
    Arc::new(config)
}

/// Host side: a socket ready for punching and the certificate the viewer pins.
pub struct PunchHost {
    gathered: Gathered,
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl PunchHost {
    pub async fn prepare(server: &str) -> Result<Self> {
        let gathered = gather(server).await?;
        let certified = rcgen::generate_simple_self_signed(vec![SERVER_NAME.into()])?;
        let cert = certified.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der());
        Ok(Self { gathered, cert, key })
    }

    pub fn candidates(&self) -> &[String] {
        &self.gathered.candidates
    }

    /// SHA-256 of the certificate, sent to the viewer inside the session.
    pub fn cert_hash(&self) -> [u8; 32] {
        Sha256::digest(&self.cert).into()
    }

    /// Punches towards the viewer's candidates and accepts its QUIC
    /// connection. The result still has to pass the token check.
    pub async fn accept(self, viewer: &[String]) -> Result<Transport> {
        let peers = parse_candidates(viewer);
        if peers.is_empty() {
            bail!("keine Adressen des Viewers");
        }
        probe(&self.gathered.socket, &peers).await;

        let mut tls = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(vec![self.cert], self.key.into())?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
        server.transport_config(transport_config());
        let socket = self.gathered.socket.into_std()?;
        let endpoint = Endpoint::new(EndpointConfig::default(), Some(server), socket, Arc::new(TokioRuntime))?;

        let result = timeout(PUNCH_TIMEOUT, async {
            let incoming = endpoint.accept().await.context("Endpunkt geschlossen")?;
            // One viewer per offer; later handshakes are refused.
            endpoint.set_server_config(None);
            let connection = incoming.await?;
            let (send, recv) = connection.accept_bi().await?;
            anyhow::Ok(QuicIo::new(send, recv, connection, endpoint.clone()))
        })
        .await;
        match result {
            Ok(Ok(io)) => Ok(framing::transport_over(io)),
            Ok(Err(e)) => {
                endpoint.close(0u8.into(), b"");
                Err(e)
            }
            Err(_) => {
                endpoint.close(0u8.into(), b"");
                bail!("Zeitüberschreitung beim Durchstoßen des NAT")
            }
        }
    }
}

/// Viewer side: a socket ready for punching.
pub struct PunchViewer {
    gathered: Gathered,
}

impl PunchViewer {
    pub async fn prepare(server: &str) -> Result<Self> {
        Ok(Self { gathered: gather(server).await? })
    }

    pub fn candidates(&self) -> &[String] {
        &self.gathered.candidates
    }

    /// Connects to the host whose certificate hashes to `cert` and presents
    /// `token`. Returns the confirmed connection and the address that worked.
    pub async fn dial(self, host: &[String], cert: [u8; 32], token: [u8; 32]) -> Result<(Transport, String)> {
        let peers = parse_candidates(host);
        if peers.is_empty() {
            bail!("keine Adressen des Hosts");
        }
        probe(&self.gathered.socket, &peers).await;

        let mut tls = rustls::ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned { hash: cert, provider: provider() }))
            .with_no_client_auth();
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut client = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?));
        client.transport_config(transport_config());
        let socket = self.gathered.socket.into_std()?;
        let mut endpoint = Endpoint::new(EndpointConfig::default(), None, socket, Arc::new(TokioRuntime))?;
        endpoint.set_default_client_config(client);

        let result = timeout(PUNCH_TIMEOUT, async {
            let mut attempts: FuturesUnordered<_> = peers
                .iter()
                .filter_map(|peer| {
                    let connecting = endpoint.connect(*peer, SERVER_NAME).ok()?;
                    Some(async move { (*peer, connecting.await) })
                })
                .collect();
            while let Some((peer, connected)) = attempts.next().await {
                match connected {
                    Ok(connection) => match confirm(connection, endpoint.clone(), token).await {
                        Ok(t) => return Ok((t, peer.to_string())),
                        Err(e) => debug!(%peer, "QUIC-Verbindung abgelehnt: {e:#}"),
                    },
                    Err(e) => debug!(%peer, "QUIC-Verbindung nicht möglich: {e}"),
                }
            }
            bail!("keine Adresse erreichbar")
        })
        .await;
        match result {
            Ok(Ok(connected)) => Ok(connected),
            Ok(Err(e)) => {
                endpoint.close(0u8.into(), b"");
                Err(e)
            }
            Err(_) => {
                endpoint.close(0u8.into(), b"");
                bail!("Zeitüberschreitung beim Durchstoßen des NAT")
            }
        }
    }
}

/// Opens the stream and runs the same token check as a TCP direct connection.
async fn confirm(connection: Connection, endpoint: Endpoint, token: [u8; 32]) -> Result<Transport> {
    let (send, recv) = connection.open_bi().await?;
    let mut t = framing::transport_over(QuicIo::new(send, recv, connection, endpoint));
    t.codec_mut().set_max_frame_length(256);
    framing::send(&mut t, &DirectHello { token }).await?;
    let echo: DirectHello = framing::recv(&mut t).await?;
    if echo.token != token {
        bail!("falsche Gegenstelle");
    }
    t.codec_mut().set_max_frame_length(MAX_FRAME);
    Ok(t)
}

/// A QUIC stream as one byte stream; keeps its connection and endpoint alive.
struct QuicIo {
    io: Join<RecvStream, SendStream>,
    _connection: Connection,
    _endpoint: Endpoint,
}

impl QuicIo {
    fn new(send: SendStream, recv: RecvStream, connection: Connection, endpoint: Endpoint) -> Self {
        Self { io: tokio::io::join(recv, send), _connection: connection, _endpoint: endpoint }
    }
}

impl AsyncRead for QuicIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// Accepts exactly the certificate whose hash came inside the encrypted session.
#[derive(Debug)]
struct Pinned {
    hash: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let hash: [u8; 32] = Sha256::digest(end_entity).into();
        if hash == self.hash {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("unerwartetes Zertifikat".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reflector like the server's, on localhost.
    async fn reflector() -> String {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                if let Some(reply) = reflect::answer(&buf[..len], from) {
                    let _ = socket.send_to(&reply, from).await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn punched_connection_carries_frames() {
        let server = reflector().await;
        let host = PunchHost::prepare(&server).await.unwrap();
        let viewer = PunchViewer::prepare(&server).await.unwrap();
        // On localhost the reflected address is the socket itself.
        assert!(host.candidates()[0].starts_with("127.0.0.1:"));
        let (host_candidates, viewer_candidates) = (host.candidates().to_vec(), viewer.candidates().to_vec());
        let cert = host.cert_hash();
        let token = [3; 32];

        let accepting = tokio::spawn(async move {
            let mut t = host.accept(&viewer_candidates).await.unwrap();
            // The listener's token check, done by hand here.
            let hello: DirectHello = framing::recv(&mut t).await.unwrap();
            framing::send(&mut t, &hello).await.unwrap();
            assert_eq!(framing::recv::<u32>(&mut t).await.unwrap(), 7);
            framing::send(&mut t, &vec![9u8; 3 * 1024 * 1024]).await.unwrap();
            // Keeps the connection open until the viewer has read everything.
            let _ = framing::recv::<u32>(&mut t).await;
        });
        let (mut t, addr) = viewer.dial(&host_candidates, cert, token).await.unwrap();
        assert!(host_candidates.contains(&addr));
        framing::send(&mut t, &7u32).await.unwrap();
        let big: Vec<u8> = framing::recv(&mut t).await.unwrap();
        assert_eq!(big.len(), 3 * 1024 * 1024);
        drop(t);
        accepting.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_certificate_is_refused() {
        let server = reflector().await;
        let host = PunchHost::prepare(&server).await.unwrap();
        let viewer = PunchViewer::prepare(&server).await.unwrap();
        let host_candidates = host.candidates().to_vec();
        let viewer_candidates = viewer.candidates().to_vec();
        tokio::spawn(async move {
            let _ = host.accept(&viewer_candidates).await;
        });
        let result = timeout(Duration::from_secs(15), viewer.dial(&host_candidates, [0; 32], [3; 32])).await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn silent_server_fails_fast() {
        // A bound socket that never answers.
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = silent.local_addr().unwrap().to_string();
        assert!(PunchViewer::prepare(&addr).await.is_err());
    }
}
