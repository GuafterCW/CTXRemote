//! Password-authenticated, end-to-end encrypted channel between viewer and host.
//!
//! Both peers run SPAKE2 with the host's password, so a relay that does not
//! know the password learns nothing and gets one online guess per password
//! slot and session. The shared secret is expanded into one ChaCha20-Poly1305 key per
//! direction; nonces are per-direction frame counters and never repeat.
//!
//! The host offers one SPAKE2 exchange per accepted password (one-time and
//! permanent). The viewer answers every slot with its single password and
//! proves knowledge of each resulting key by sealing a fixed confirmation
//! value; the host picks the slot that opens. A wrong password therefore shows
//! up as "no slot matched", never as a broken connection.

use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use hkdf::Hkdf;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::framing::{self, Transport};
use crate::{DeviceId, PROTOCOL_VERSION};

const CONFIRM: &[u8] = b"ctxremote/confirm/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum Refusal {
    #[error("Falsches Passwort")]
    WrongPassword,
    #[error("Zu viele Fehlversuche, bitte später erneut versuchen")]
    LockedOut,
    #[error("Die Verbindung wurde am Gerät abgelehnt")]
    Declined,
    #[error("Inkompatible Programmversion")]
    Version,
}

#[derive(Debug, Serialize, Deserialize)]
enum Handshake {
    /// One SPAKE2 message per password slot the host accepts.
    Hello { version: u16, spake: Vec<Vec<u8>> },
    /// Viewer → host: the confirmation value sealed under each slot's key.
    Confirm { proofs: Vec<Vec<u8>> },
    /// Host → viewer: the slot that matched, proven the same way.
    Accepted { slot: u8, proof: Vec<u8> },
    Refused(Refusal),
}

/// Sends a plaintext refusal before any key exchange, e.g. while locked out.
pub async fn refuse(t: &mut Transport, reason: Refusal) -> Result<()> {
    framing::send(t, &Handshake::Refused(reason)).await
}

struct SlotKeys {
    host_to_viewer: [u8; 32],
    viewer_to_host: [u8; 32],
    binding: [u8; 32],
}

fn start(host_id: DeviceId, password: &str) -> (Spake2<Ed25519Group>, Vec<u8>) {
    let identity = format!("ctxremote/v1/{}", host_id.get());
    Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(password.as_bytes()),
        &Identity::new(identity.as_bytes()),
    )
}

fn finish(state: Spake2<Ed25519Group>, inbound: &[u8]) -> Result<SlotKeys> {
    let shared = state
        .finish(inbound)
        .map_err(|e| anyhow!("Schlüsselaustausch fehlgeschlagen: {e:?}"))?;
    let hk = Hkdf::<Sha256>::new(Some(b"ctxremote/v1"), &shared);
    let mut keys = SlotKeys { host_to_viewer: [0; 32], viewer_to_host: [0; 32], binding: [0; 32] };
    hk.expand(b"host->viewer", &mut keys.host_to_viewer).expect("valid length");
    hk.expand(b"viewer->host", &mut keys.viewer_to_host).expect("valid length");
    hk.expand(b"binding", &mut keys.binding).expect("valid length");
    Ok(keys)
}

fn seal_confirm(key: &[u8; 32]) -> Vec<u8> {
    cipher(key).encrypt(&nonce(0), CONFIRM).expect("in-memory encryption cannot fail")
}

fn opens_confirm(key: &[u8; 32], proof: &[u8]) -> bool {
    cipher(key).decrypt(&nonce(0), proof).is_ok_and(|plain| plain == CONFIRM)
}

/// Splits the transport into an encrypted pair whose counters continue after
/// the confirmation frames (nonce 0 in each direction).
fn channel(t: Transport, tx_key: &[u8; 32], rx_key: &[u8; 32], binding: [u8; 32]) -> (SecureSender, SecureReceiver) {
    let (sink, stream) = t.split();
    (
        SecureSender { sink, cipher: cipher(tx_key), counter: 1 },
        SecureReceiver { stream, cipher: cipher(rx_key), counter: 1, binding },
    )
}

/// Host side. `passwords` are the accepted passwords in slot order (e.g. the
/// one-time password, then the permanent one). Returns the matching slot.
pub async fn host_handshake(
    mut t: Transport,
    host_id: DeviceId,
    passwords: &[&str],
) -> Result<(SecureSender, SecureReceiver, usize)> {
    let (states, outbound): (Vec<_>, Vec<_>) = passwords.iter().map(|pw| start(host_id, pw)).unzip();
    framing::send(&mut t, &Handshake::Hello { version: PROTOCOL_VERSION, spake: outbound }).await?;

    let inbound = match framing::recv::<Handshake>(&mut t).await? {
        Handshake::Hello { version, .. } if version != PROTOCOL_VERSION => {
            return Err(Refusal::Version.into())
        }
        Handshake::Hello { spake, .. } if spake.len() == states.len() => spake,
        _ => bail!("unerwartete Handshake-Nachricht"),
    };
    let Handshake::Confirm { proofs } = framing::recv::<Handshake>(&mut t).await? else {
        bail!("unerwartete Handshake-Nachricht");
    };

    let mut matched = None;
    for (slot, ((state, msg), proof)) in states.into_iter().zip(&inbound).zip(&proofs).enumerate() {
        let keys = finish(state, msg)?;
        if matched.is_none() && opens_confirm(&keys.viewer_to_host, proof) {
            matched = Some((slot, keys));
        }
    }
    let Some((slot, keys)) = matched else {
        refuse(&mut t, Refusal::WrongPassword).await?;
        return Err(Refusal::WrongPassword.into());
    };
    let proof = seal_confirm(&keys.host_to_viewer);
    framing::send(&mut t, &Handshake::Accepted { slot: slot as u8, proof }).await?;
    let (tx, rx) = channel(t, &keys.host_to_viewer, &keys.viewer_to_host, keys.binding);
    Ok((tx, rx, slot))
}

/// Viewer side. Errors that are a [`Refusal`] can be downcast for display.
pub async fn viewer_handshake(
    mut t: Transport,
    host_id: DeviceId,
    password: &str,
) -> Result<(SecureSender, SecureReceiver)> {
    let host_msgs = match framing::recv::<Handshake>(&mut t).await? {
        Handshake::Hello { version, .. } if version != PROTOCOL_VERSION => {
            return Err(Refusal::Version.into())
        }
        Handshake::Hello { spake, .. } if !spake.is_empty() && spake.len() <= 8 => spake,
        Handshake::Refused(reason) => return Err(reason.into()),
        _ => bail!("unerwartete Handshake-Nachricht"),
    };

    let mut outbound = Vec::with_capacity(host_msgs.len());
    let mut slots = Vec::with_capacity(host_msgs.len());
    for msg in &host_msgs {
        let (state, out) = start(host_id, password);
        outbound.push(out);
        slots.push(finish(state, msg)?);
    }
    let proofs = slots.iter().map(|k| seal_confirm(&k.viewer_to_host)).collect();
    framing::send(&mut t, &Handshake::Hello { version: PROTOCOL_VERSION, spake: outbound }).await?;
    framing::send(&mut t, &Handshake::Confirm { proofs }).await?;

    match framing::recv::<Handshake>(&mut t).await? {
        Handshake::Accepted { slot, proof } => {
            let keys = slots.get(slot as usize).context("ungültiger Slot")?;
            if !opens_confirm(&keys.host_to_viewer, &proof) {
                bail!("Gegenstelle konnte sich nicht ausweisen");
            }
            Ok(channel(t, &keys.viewer_to_host, &keys.host_to_viewer, keys.binding))
        }
        Handshake::Refused(reason) => Err(reason.into()),
        _ => bail!("unerwartete Handshake-Nachricht"),
    }
}

fn cipher(key: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(key))
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(n)
}

/// The writing half of a transport, e.g. for [`SecureSender::reroute`].
pub type TransportSink = SplitSink<Transport, Bytes>;
/// The reading half of a transport, e.g. for [`SecureReceiver::reroute`].
pub type TransportStream = SplitStream<Transport>;

pub struct SecureSender {
    sink: TransportSink,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl SecureSender {
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        self.send_raw(&postcard::to_stdvec(msg)?).await
    }

    /// Sends `msg` with `trailer` appended in the same frame. Receivers that only
    /// decode `msg` ignore the trailer, so it can carry optional extras.
    pub async fn send_with_trailer<T: Serialize, U: Serialize>(&mut self, msg: &T, trailer: &U) -> Result<()> {
        let mut plain = postcard::to_stdvec(msg)?;
        plain.extend_from_slice(&postcard::to_stdvec(trailer)?);
        self.send_raw(&plain).await
    }

    /// Continues on another transport and returns the old one's writing half.
    /// The counter carries on, so the peer must read the new transport strictly
    /// after everything sent on the old one.
    pub fn reroute(&mut self, sink: TransportSink) -> TransportSink {
        std::mem::replace(&mut self.sink, sink)
    }

    async fn send_raw(&mut self, plain: &[u8]) -> Result<()> {
        let sealed = self
            .cipher
            .encrypt(&nonce(self.counter), plain)
            .map_err(|_| anyhow!("Verschlüsselung fehlgeschlagen"))?;
        self.counter += 1;
        self.sink.send(Bytes::from(sealed)).await?;
        Ok(())
    }

    pub async fn close(&mut self) {
        let _ = self.sink.close().await;
    }
}

pub struct SecureReceiver {
    stream: TransportStream,
    cipher: ChaCha20Poly1305,
    counter: u64,
    binding: [u8; 32],
}

impl SecureReceiver {
    /// A value both ends derive from this session's key exchange and nobody
    /// else knows; signatures over it cannot be replayed in another session.
    pub fn binding(&self) -> [u8; 32] {
        self.binding
    }

    /// Returns `None` when the peer closed the connection cleanly.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        match self.recv_raw().await {
            Ok(plain) => Ok(Some(postcard::from_bytes(&plain)?)),
            Err(e) if e.is::<Closed>() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Like [`Self::recv`], plus the trailer if the peer sent one that decodes.
    pub async fn recv_with_trailer<T: DeserializeOwned, U: DeserializeOwned>(&mut self) -> Result<Option<(T, Option<U>)>> {
        Ok(self.recv_with_rest::<T>().await?.map(|(msg, rest)| {
            let trailer = if rest.is_empty() { None } else { postcard::from_bytes(&rest).ok() };
            (msg, trailer)
        }))
    }

    /// Like [`Self::recv`], plus the undecoded bytes after the message.
    pub async fn recv_with_rest<T: DeserializeOwned>(&mut self) -> Result<Option<(T, Vec<u8>)>> {
        let plain = match self.recv_raw().await {
            Ok(plain) => plain,
            Err(e) if e.is::<Closed>() => return Ok(None),
            Err(e) => return Err(e),
        };
        let (msg, rest) = postcard::take_from_bytes::<T>(&plain)?;
        Ok(Some((msg, rest.to_vec())))
    }

    /// Continues reading from another transport and returns the old one's
    /// reading half. Call it once the old transport delivered its last message.
    pub fn reroute(&mut self, stream: TransportStream) -> TransportStream {
        std::mem::replace(&mut self.stream, stream)
    }

    async fn recv_raw(&mut self) -> Result<Vec<u8>> {
        let frame = self.stream.next().await.ok_or(Closed)??;
        let plain = self
            .cipher
            .decrypt(&nonce(self.counter), frame.as_ref())
            .map_err(|_| anyhow!("Nachricht konnte nicht entschlüsselt werden"))?;
        self.counter += 1;
        Ok(plain)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Verbindung wurde geschlossen")]
struct Closed;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ViewerMsg;
    use tokio::net::{TcpListener, TcpStream};

    async fn pair() -> (Transport, Transport) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (a, b) = tokio::join!(TcpStream::connect(addr), listener.accept());
        (framing::transport(a.unwrap()), framing::transport(b.unwrap().0))
    }

    const ID: u32 = 123_456_789;

    #[tokio::test]
    async fn permanent_password_matches_second_slot() {
        let id = DeviceId::new(ID).unwrap();
        let (a, b) = pair().await;
        let (viewer, host) = tokio::join!(
            viewer_handshake(a, id, "permanent"),
            host_handshake(b, id, &["one-time", "permanent"]),
        );
        let (mut vtx, _vrx) = viewer.unwrap();
        let (_htx, mut hrx, slot) = host.unwrap();
        assert_eq!(slot, 1);
        vtx.send(&ViewerMsg::RequestKeyframe).await.unwrap();
        assert!(matches!(hrx.recv::<ViewerMsg>().await.unwrap(), Some(ViewerMsg::RequestKeyframe)));
    }

    #[tokio::test]
    async fn trailer_and_reroute() {
        let id = DeviceId::new(ID).unwrap();
        let (a, b) = pair().await;
        let (viewer, host) = tokio::join!(viewer_handshake(a, id, "pw"), host_handshake(b, id, &["pw"]));
        let (mut vtx, _vrx) = viewer.unwrap();
        let (_htx, mut hrx, _) = host.unwrap();

        vtx.send_with_trailer(&ViewerMsg::RequestKeyframe, &7u32).await.unwrap();
        let (msg, trailer) = hrx.recv_with_trailer::<ViewerMsg, u32>().await.unwrap().unwrap();
        assert!(matches!(msg, ViewerMsg::RequestKeyframe));
        assert_eq!(trailer, Some(7));
        vtx.send(&ViewerMsg::LockScreen).await.unwrap();
        assert!(hrx.recv_with_trailer::<ViewerMsg, u32>().await.unwrap().unwrap().1.is_none());

        // Move to a second connection mid-stream; the counters carry on.
        let (c, d) = pair().await;
        vtx.send(&ViewerMsg::Switch).await.unwrap();
        let mut old = vtx.reroute(c.split().0);
        old.close().await.unwrap();
        vtx.send(&ViewerMsg::Bye).await.unwrap();
        assert!(matches!(hrx.recv::<ViewerMsg>().await.unwrap(), Some(ViewerMsg::Switch)));
        drop(hrx.reroute(d.split().1));
        assert!(matches!(hrx.recv::<ViewerMsg>().await.unwrap(), Some(ViewerMsg::Bye)));
    }

    #[tokio::test]
    async fn wrong_password_is_reported() {
        let id = DeviceId::new(ID).unwrap();
        let (a, b) = pair().await;
        let (viewer, host) = tokio::join!(
            viewer_handshake(a, id, "guess"),
            host_handshake(b, id, &["s3cret"]),
        );
        for err in [viewer.err().unwrap(), host.err().unwrap()] {
            assert_eq!(err.downcast_ref::<Refusal>(), Some(&Refusal::WrongPassword));
        }
    }
}
