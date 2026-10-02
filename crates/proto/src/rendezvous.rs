//! Messages between a client and the rendezvous/relay server.
//!
//! Every connection begins with the server sending [`ServerMsg::Challenge`].
//! The client answers with exactly one of:
//!
//! * [`ClientMsg::Register`]: keeps the connection open as the device's
//!   control channel. The server pushes [`ServerMsg::Incoming`] on it.
//! * [`ClientMsg::Connect`]: a viewer asks to reach a device.
//! * [`ClientMsg::Join`]: a host accepts an [`ServerMsg::Incoming`] session.
//!
//! After both sides of a session receive [`ServerMsg::Ready`] the server
//! becomes a transparent byte relay. Clients must not send anything between
//! their opening message and `Ready`.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::DeviceId;

pub type Nonce = [u8; 32];
pub type SessionId = [u8; 16];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Challenge { version: u16, nonce: Nonce },
    Registered { id: DeviceId },
    Incoming { session: SessionId },
    Ready,
    Pong,
    Error(ServerError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    Register {
        /// The ID this device used before; the server keeps it if the key matches.
        id: Option<DeviceId>,
        public_key: [u8; 32],
        signature: Vec<u8>,
    },
    Connect { target: DeviceId },
    Join { session: SessionId },
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ServerError {
    #[error("Das Gerät ist nicht online")]
    Offline,
    #[error("Das Gerät hat nicht rechtzeitig geantwortet")]
    Timeout,
    #[error("Die Anmeldung am Server wurde abgelehnt")]
    BadSignature,
    #[error("Der Server verwendet eine andere Protokollversion")]
    Version,
    #[error("Zu viele Anfragen, bitte kurz warten")]
    RateLimited,
}

fn register_payload(nonce: &Nonce) -> Vec<u8> {
    [b"ctxremote/register/v1:".as_slice(), nonce].concat()
}

pub fn sign_challenge(key: &SigningKey, nonce: &Nonce) -> Vec<u8> {
    key.sign(&register_payload(nonce)).to_bytes().to_vec()
}

pub fn verify_challenge(public_key: &[u8; 32], nonce: &Nonce, signature: &[u8]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify(&register_payload(nonce), &sig).is_ok()
}
