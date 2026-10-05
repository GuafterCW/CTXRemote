//! Messages between a client and the rendezvous/relay server.
//!
//! Every connection begins with the server sending [`ServerMsg::Challenge`].
//! The client answers with exactly one of:
//!
//! * [`ClientMsg::Register`]: keeps the connection open as the device's
//!   control channel. The server pushes [`ServerMsg::Incoming`] on it.
//! * [`ClientMsg::Connect`]: a viewer asks to reach a device.
//! * [`ClientMsg::Join`]: a host accepts an [`ServerMsg::Incoming`] session.
//! * [`ClientMsg::ClaimAlias`]: a device sets (or drops) its public alias,
//!   proving with its key that it owns its ID. [`ClientMsg::ResolveAlias`]
//!   looks an alias up. Servers from before aliases close the connection.
//! * [`ClientMsg::Account`]: an account request (address-book sync, pairing),
//!   see [`crate::account`].
//! * [`ClientMsg::UpdateCheck`] / [`ClientMsg::UpdateDownload`]: asks for the
//!   newest client release (see [`crate::update`]). Servers from before this
//!   existed close the connection, which clients take as "no update".
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
    /// Answer to `UpdateCheck`: the newest release for that platform, if any.
    Update(Option<crate::update::UpdateInfo>),
    /// Part of the installer requested with `UpdateDownload`.
    UpdateData(Vec<u8>),
    /// The installer is complete.
    UpdateEnd,
    /// Answer to `ClaimAlias`: the alias as stored (normalised), `None` if dropped.
    AliasClaimed(Result<Option<String>, AliasError>),
    /// Answer to `ResolveAlias`.
    AliasResolved(Option<DeviceId>),
    /// Answer to `Account`.
    Account(Result<crate::account::AccountReply, crate::account::AccountError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum AliasError {
    #[error("Ein Alias hat 3 bis 32 Zeichen: Kleinbuchstaben, Ziffern, Punkt, Binde- oder Unterstrich, mindestens ein Buchstabe")]
    Invalid,
    #[error("Dieser Alias ist schon vergeben")]
    Taken,
    #[error("Das Gerät ist am Server nicht bekannt")]
    UnknownDevice,
    #[error("Die Anmeldung am Server wurde abgelehnt")]
    BadSignature,
}

/// Normalises a public alias: trimmed and lowercase. 3–32 characters from
/// `a-z0-9.-_`, starting and ending with a letter or digit, with at least one
/// letter so an alias never reads like a device ID.
pub fn normalize_alias(alias: &str) -> Result<String, AliasError> {
    let alias = alias.trim().to_lowercase();
    let len = alias.chars().count();
    let allowed = alias.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    let edges = alias.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && alias.chars().last().is_some_and(|c| c.is_ascii_alphanumeric());
    let letter = alias.chars().any(|c| c.is_ascii_lowercase());
    if (3..=32).contains(&len) && allowed && edges && letter {
        Ok(alias)
    } else {
        Err(AliasError::Invalid)
    }
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
    UpdateCheck { platform: String },
    /// Sets the public alias of device `id` (`None` drops it). Signed like
    /// `Register`, but over [`sign_alias_claim`]'s payload.
    ClaimAlias { id: DeviceId, public_key: [u8; 32], signature: Vec<u8>, alias: Option<String> },
    ResolveAlias { alias: String },
    /// Fetches the release announced for `platform`, if it is still `version`.
    UpdateDownload { platform: String, version: String },
    /// An account request, signed over the nonce and `op` (see [`crate::account`]).
    /// Servers from before accounts close the connection.
    Account { auth: crate::account::DeviceAuth, op: crate::account::AccountOp },
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
    #[error("Dieses Update ist auf dem Server nicht mehr vorhanden")]
    UpdateGone,
}

fn register_payload(nonce: &Nonce) -> Vec<u8> {
    [b"ctxremote/register/v1:".as_slice(), nonce].concat()
}

pub fn sign_challenge(key: &SigningKey, nonce: &Nonce) -> Vec<u8> {
    key.sign(&register_payload(nonce)).to_bytes().to_vec()
}

fn alias_payload(nonce: &Nonce, alias: Option<&str>) -> Vec<u8> {
    [b"ctxremote/alias/v1:".as_slice(), nonce, alias.unwrap_or("").as_bytes()].concat()
}

/// Proves key ownership for `ClaimAlias`. Separate from the registration
/// payload, so a registration signature can never be replayed as a claim.
pub fn sign_alias_claim(key: &SigningKey, nonce: &Nonce, alias: Option<&str>) -> Vec<u8> {
    key.sign(&alias_payload(nonce, alias)).to_bytes().to_vec()
}

pub fn verify_alias_claim(public_key: &[u8; 32], nonce: &Nonce, alias: Option<&str>, signature: &[u8]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else { return false };
    let Ok(sig) = Signature::from_slice(signature) else { return false };
    key.verify(&alias_payload(nonce, alias), &sig).is_ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_rules() {
        assert_eq!(normalize_alias("  Philipp-PC ").unwrap(), "philipp-pc");
        assert_eq!(normalize_alias("buero.laptop_2").unwrap(), "buero.laptop_2");
        for bad in ["ab", "123456789", "-abc", "abc.", "a b c", "über", "a/b", &"x".repeat(33)] {
            assert!(normalize_alias(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn alias_claims_do_not_mix_with_registration() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let public = key.verifying_key().to_bytes();
        let nonce = [5; 32];
        let claim = sign_alias_claim(&key, &nonce, Some("philipp"));
        assert!(verify_alias_claim(&public, &nonce, Some("philipp"), &claim));
        assert!(!verify_alias_claim(&public, &nonce, Some("anderer"), &claim));
        assert!(!verify_alias_claim(&public, &nonce, None, &claim));
        let register = sign_challenge(&key, &nonce);
        assert!(!verify_alias_claim(&public, &nonce, None, &register));
        assert!(!verify_challenge(&public, &nonce, &claim));
    }
}
