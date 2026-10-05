//! Encrypted connections between clients and the server.
//!
//! The server has a static X25519 key whose public half is built into the
//! clients (`CTXREMOTE_SERVER_KEY`), like the update key. After the plaintext
//! `Challenge`, the client sends [`ClientMsg::Tunnel`] with an ephemeral key;
//! the server answers [`ServerMsg::Tunnel`] with its own. Both derive two keys
//! from ephemeral–ephemeral and ephemeral–static Diffie-Hellman (the Noise NK
//! pattern): only the holder of the server's private key can read the client's
//! traffic, and an attacker in between learns nothing, not even the device
//! IDs. Everything after that, including a relayed session (which is
//! end-to-end encrypted on top), runs through [`SecureIo`], starting with a
//! fresh encrypted `Challenge` whose nonce the client signs from then on.
//!
//! [`SecureIo`] is a record layer below the length-delimited framing, so the
//! rest of the protocol and the server's byte relay do not change.

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use anyhow::{bail, Context as _, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::framing::{self, Conn, Transport};
use crate::rendezvous::{ClientMsg, Nonce as ChallengeNonce, ServerMsg};
use crate::PROTOCOL_VERSION;

/// Plaintext bytes per record at most.
const RECORD_MAX: usize = 16 * 1024;
const TAG: usize = 16;

/// The two directions' keys: client → server and server → client.
fn derive(
    ee: &[u8; 32],
    es: &[u8; 32],
    nonce: &ChallengeNonce,
    client: &[u8; 32],
    server_ephemeral: &[u8; 32],
    server_static: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    let ikm = [ee.as_slice(), es].concat();
    let info = [nonce.as_slice(), client, server_ephemeral, server_static].concat();
    let mut okm = [0u8; 64];
    Hkdf::<Sha256>::new(Some(b"ctxremote/tunnel/v1"), &ikm)
        .expand(&info, &mut okm)
        .expect("64 bytes is a valid HKDF length");
    let (up, down) = okm.split_at(32);
    (up.try_into().expect("32"), down.try_into().expect("32"))
}

fn dh(secret: &StaticSecret, public: &[u8; 32]) -> Result<[u8; 32]> {
    let shared = secret.diffie_hellman(&PublicKey::from(*public));
    // A low-order point would make the shared secret predictable.
    if !shared.was_contributory() {
        bail!("ungültiger Schlüssel der Gegenstelle");
    }
    Ok(shared.to_bytes())
}

/// Client side, right after the plaintext challenge with `nonce`. Returns the
/// encrypted transport and the encrypted challenge's nonce, which signatures
/// must use from now on.
pub async fn client(mut t: Transport, server_static: &[u8; 32], nonce: ChallengeNonce) -> Result<(Transport, ChallengeNonce)> {
    let ephemeral = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&ephemeral).to_bytes();
    framing::send(&mut t, &ClientMsg::Tunnel { ephemeral: public }).await?;
    let server_ephemeral = match framing::recv::<ServerMsg>(&mut t).await {
        Ok(ServerMsg::Tunnel { ephemeral }) => ephemeral,
        Ok(other) => bail!("unerwartete Serverantwort: {other:?}"),
        Err(_) => bail!("Der Server verschlüsselt noch nicht (Server-Update nötig)"),
    };
    let ee = dh(&ephemeral, &server_ephemeral)?;
    let es = dh(&ephemeral, server_static)?;
    let (up, down) = derive(&ee, &es, &nonce, &public, &server_ephemeral, server_static);
    let mut t = rewrap(t, up, down);
    // Only a server with the private key can produce this.
    match framing::recv::<ServerMsg>(&mut t).await.context("Der Server hat sich nicht als der erwartete ausgewiesen")? {
        ServerMsg::Challenge { version, .. } if version != PROTOCOL_VERSION => {
            bail!("Server verwendet Protokoll {version}, erwartet {PROTOCOL_VERSION}")
        }
        ServerMsg::Challenge { nonce, .. } => Ok((t, nonce)),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Server side, after the client's [`ClientMsg::Tunnel`] on the connection
/// that was challenged with `nonce`. Sends the encrypted challenge with
/// `next_nonce` and returns the encrypted transport.
pub async fn server(
    mut t: Transport,
    secret: &StaticSecret,
    client_ephemeral: &[u8; 32],
    nonce: ChallengeNonce,
    next_nonce: ChallengeNonce,
) -> Result<Transport> {
    let ephemeral = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&ephemeral).to_bytes();
    let ee = dh(&ephemeral, client_ephemeral)?;
    let es = dh(secret, client_ephemeral)?;
    let server_static = PublicKey::from(secret).to_bytes();
    let (up, down) = derive(&ee, &es, &nonce, client_ephemeral, &public, &server_static);
    framing::send(&mut t, &ServerMsg::Tunnel { ephemeral: public }).await?;
    // The server reads what the client sends up and writes down.
    let mut t = rewrap(t, down, up);
    framing::send(&mut t, &ServerMsg::Challenge { version: PROTOCOL_VERSION, nonce: next_nonce }).await?;
    Ok(t)
}

/// Continues `t` through a [`SecureIo`], keeping bytes already read.
fn rewrap(t: Transport, send: [u8; 32], recv: [u8; 32]) -> Transport {
    let parts = t.into_parts();
    framing::transport_over(SecureIo::new(parts.io, send, recv, parts.read_buf.to_vec()))
}

/// A byte stream in encrypted records: `[length u32 BE][ChaCha20-Poly1305]`,
/// nonces counting up per direction. Any tampering ends the connection.
pub struct SecureIo<T> {
    inner: T,
    send: ChaCha20Poly1305,
    send_counter: u64,
    recv: ChaCha20Poly1305,
    recv_counter: u64,
    /// Encrypted bytes not yet written.
    out: Vec<u8>,
    /// Raw bytes read but not yet decrypted.
    raw: Vec<u8>,
    /// Decrypted bytes not yet handed out.
    plain: Vec<u8>,
    plain_pos: usize,
}

impl SecureIo<Conn> {
    fn new(inner: Conn, send: [u8; 32], recv: [u8; 32], raw: Vec<u8>) -> Self {
        Self {
            inner,
            send: ChaCha20Poly1305::new((&send).into()),
            send_counter: 0,
            recv: ChaCha20Poly1305::new((&recv).into()),
            recv_counter: 0,
            out: Vec::new(),
            raw,
            plain: Vec::new(),
            plain_pos: 0,
        }
    }
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    n.into()
}

fn broken(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

impl<T: AsyncWrite + Unpin> SecureIo<T> {
    /// Writes pending records; `Ready` once all are out.
    fn drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.out.is_empty() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.out.drain(..n);
        }
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for SecureIo<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        // Bounded buffering: earlier records go out before new ones are taken.
        ready!(self.drain(cx))?;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let take = buf.len().min(RECORD_MAX);
        let counter = self.send_counter;
        self.send_counter = counter.checked_add(1).ok_or_else(|| broken("Verbindung zu lang"))?;
        let sealed = self.send.encrypt(&nonce(counter), &buf[..take]).map_err(|_| broken("Verschlüsselung fehlgeschlagen"))?;
        self.out.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
        self.out.extend_from_slice(&sealed);
        // Best effort now; the rest goes out on the next write or flush.
        let _ = self.drain(cx)?;
        Poll::Ready(Ok(take))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.drain(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.drain(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for SecureIo<T> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.plain_pos < self.plain.len() {
                let n = buf.remaining().min(self.plain.len() - self.plain_pos);
                let start = self.plain_pos;
                buf.put_slice(&self.plain[start..start + n]);
                self.plain_pos += n;
                return Poll::Ready(Ok(()));
            }
            if self.raw.len() >= 4 {
                let len = u32::from_be_bytes(self.raw[..4].try_into().expect("4 bytes")) as usize;
                if !(TAG + 1..=RECORD_MAX + TAG).contains(&len) {
                    return Poll::Ready(Err(broken("ungültiger Datensatz")));
                }
                if self.raw.len() >= 4 + len {
                    let counter = self.recv_counter;
                    self.recv_counter = counter.checked_add(1).ok_or_else(|| broken("Verbindung zu lang"))?;
                    let plain = self
                        .recv
                        .decrypt(&nonce(counter), &self.raw[4..4 + len])
                        .map_err(|_| broken("Daten wurden unterwegs verändert"))?;
                    self.raw.drain(..4 + len);
                    self.plain = plain;
                    self.plain_pos = 0;
                    continue;
                }
            }
            let mut chunk = [0u8; 16 * 1024];
            let mut read = ReadBuf::new(&mut chunk);
            ready!(Pin::new(&mut self.inner).poll_read(cx, &mut read))?;
            if read.filled().is_empty() {
                // End of stream: clean only between records.
                return if self.raw.is_empty() {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()))
                };
            }
            let filled = read.filled().to_vec();
            self.raw.extend_from_slice(&filled);
        }
    }
}

/// The public half of a server key, as built into clients.
pub fn public_key(secret: &StaticSecret) -> [u8; 32] {
    PublicKey::from(secret).to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pair() -> (Transport, Transport) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (a, b) = tokio::join!(tokio::net::TcpStream::connect(addr), listener.accept());
        (framing::transport(a.unwrap()), framing::transport(b.unwrap().0))
    }

    /// Runs the handshake the way the server and `net::dial` do.
    async fn handshake(server_secret: StaticSecret, expected: [u8; 32]) -> Result<(Transport, Transport)> {
        let (c, mut s) = pair().await;
        let nonce = [1u8; 32];
        let server = tokio::spawn(async move {
            let ClientMsg::Tunnel { ephemeral } = framing::recv::<ClientMsg>(&mut s).await? else { bail!("kein Tunnel") };
            server(s, &server_secret, &ephemeral, nonce, [2; 32]).await
        });
        let (client_t, next) = client(c, &expected, nonce).await?;
        assert_eq!(next, [2; 32]);
        Ok((client_t, server.await??))
    }

    #[tokio::test]
    async fn encrypted_both_ways_large_and_small() {
        let secret = StaticSecret::from([9u8; 32]);
        let (mut c, mut s) = handshake(secret.clone(), public_key(&secret)).await.unwrap();
        framing::send(&mut c, &ClientMsg::Ping).await.unwrap();
        assert!(matches!(framing::recv::<ClientMsg>(&mut s).await.unwrap(), ClientMsg::Ping));
        // Larger than one record, in both directions.
        let big = vec![7u8; 200_000];
        framing::send(&mut s, &big).await.unwrap();
        assert_eq!(framing::recv::<Vec<u8>>(&mut c).await.unwrap(), big);
        framing::send(&mut c, &big).await.unwrap();
        assert_eq!(framing::recv::<Vec<u8>>(&mut s).await.unwrap(), big);
    }

    #[tokio::test]
    async fn wrong_server_key_is_detected() {
        let secret = StaticSecret::from([9u8; 32]);
        let other = public_key(&StaticSecret::from([8u8; 32]));
        let err = match handshake(secret, other).await {
            Ok(_) => panic!("Handshake mit falschem Schlüssel gelungen"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("nicht als der erwartete"), "{err}");
    }
}
