//! Accounts without passwords: a set of device keys that share an address book.
//!
//! A device creates an account; more devices join with a one-time pairing
//! code shown on a member (see `docs/ACCOUNTS.md`). Every request is signed
//! with the device's Ed25519 key over the connection's nonce and the request
//! itself, so nothing secret crosses the (unencrypted) server connection.
//!
//! The address book is encrypted on the devices with a key only members know;
//! the server stores an opaque blob. A pairing hands that key over sealed with
//! a key derived from the code's secret half, which the server never sees.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::rendezvous::Nonce;

/// Characters of pairing codes: Crockford's base32 (no I, L, O, U).
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters that name the pairing on the server (not secret).
pub const CODE_ID_LEN: usize = 4;
/// Characters only the devices know; they protect the address-book key.
pub const CODE_SECRET_LEN: usize = 8;
/// Largest encrypted address book the server stores.
pub const MAX_BOOK: usize = 256 * 1024;
/// Most devices in one account.
pub const MAX_DEVICES: usize = 20;
/// How long a pairing code stays valid.
pub const PAIRING_SECS: u32 = 10 * 60;

/// Proves the request comes from the holder of `public_key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAuth {
    pub public_key: [u8; 32],
    /// Ed25519 over [`signed_payload`].
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountOp {
    /// Creates an account with this device as its first member.
    Create,
    /// Which account this device belongs to, if any.
    Status,
    /// Starts a pairing (only members): another device may join with the code
    /// whose first part is `code_id`, proving the rest with `verifier`.
    /// `sealed` is the address-book key, encrypted for that device.
    OfferPairing { code_id: String, salt: [u8; 16], verifier: [u8; 32], sealed: Vec<u8> },
    /// The salt of a pairing, needed to derive the proof from the code.
    PairingSalt { code_id: String },
    /// Joins the account of the pairing; `proof` must match its verifier.
    JoinPairing { code_id: String, proof: [u8; 32] },
    GetBook,
    /// Replaces the address book, if it is still at revision `base`.
    PutBook { base: u64, blob: Vec<u8> },
    /// Removes this device from its account; the last one deletes it.
    Leave,
    /// Salt and KDF settings for logging in as `email`. Unknown addresses get
    /// a made-up but stable salt, so this reveals nothing.
    Prelogin { email: String },
    /// Creates an account with a login, this device as its first member.
    Register { login: LoginSetup },
    /// Adds or replaces the login of this device's account (new password,
    /// new address, or a login for an account made by pairing).
    SetLogin { login: LoginSetup },
    /// Joins the account of `email`; `auth` is derived from the password.
    Login { email: String, auth: [u8; 32] },
    /// Joins the account of `email` with the recovery code instead of the
    /// password; the device should set a new password right after.
    Recover { email: String, recovery_auth: [u8; 32] },
    /// The login of this device's account, if it has one.
    LoginStatus,
    /// The devices of this device's account.
    Devices,
    /// Removes another device from this device's account.
    RemoveDevice { public_key: [u8; 32] },
    /// This device's name for the others, encrypted with the account key.
    SetLabel { label: Vec<u8> },
}

/// Everything the server keeps for a login. The password and the account key
/// never leave the devices: `auth` is derived from the password separately
/// from the key that seals the account key into `wrapped` (see `docs/ACCOUNTS.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginSetup {
    pub email: String,
    pub salt: [u8; 16],
    pub kdf: Kdf,
    pub auth: [u8; 32],
    pub wrapped: Vec<u8>,
    pub recovery_auth: [u8; 32],
    pub recovery_wrapped: Vec<u8>,
}

/// Argon2id settings of a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Kdf {
    /// For new logins: about a second in a browser, much less in the app.
    pub const CURRENT: Self = Self { memory_kib: 64 * 1024, iterations: 3, parallelism: 1 };

    /// Whether these settings are within what clients accept, so a server
    /// cannot make them weak or absurdly expensive.
    pub fn acceptable(&self) -> bool {
        (19 * 1024..=1024 * 1024).contains(&self.memory_kib)
            && (2..=10).contains(&self.iterations)
            && (1..=4).contains(&self.parallelism)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginInfo {
    pub email: String,
    /// The address was confirmed through the link in the mail.
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub public_key: [u8; 32],
    /// Its ID, if it is registered as a device.
    pub device: Option<crate::DeviceId>,
    pub online: bool,
    /// Its name, encrypted with the account key; empty if not set.
    pub label: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountReply {
    /// `None`: this device is in no account.
    Status(Option<AccountInfo>),
    Created(AccountInfo),
    PairingOffered,
    Salt([u8; 16]),
    Joined { info: AccountInfo, sealed: Vec<u8> },
    /// The stored address book; empty with revision 0 if there is none yet.
    Book { revision: u64, blob: Vec<u8> },
    Stored { revision: u64 },
    Left,
    Prelogin { salt: [u8; 16], kdf: Kdf },
    /// After `Register`, `Login` or `Recover`: the account key as sealed for
    /// the password (or for the recovery code after `Recover`).
    LoggedIn { info: AccountInfo, wrapped: Vec<u8> },
    LoginStatus(Option<LoginInfo>),
    Devices(Vec<Member>),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Number of devices in the account.
    pub devices: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum AccountError {
    #[error("Dieses Gerät gehört zu keinem Konto")]
    NotLinked,
    #[error("Dieses Gerät gehört schon zu einem Konto")]
    AlreadyLinked,
    #[error("Die Anmeldung am Server wurde abgelehnt")]
    BadSignature,
    #[error("Diesen Code gibt es nicht oder er ist abgelaufen")]
    UnknownCode,
    #[error("Der Code ist falsch")]
    WrongCode,
    #[error("Das Adressbuch wurde inzwischen auf einem anderen Gerät geändert")]
    Conflict,
    #[error("Das Adressbuch ist zu groß")]
    TooLarge,
    #[error("Das Konto hat schon die größte Zahl an Geräten")]
    TooManyDevices,
    #[error("Zu viele Anfragen, bitte kurz warten")]
    RateLimited,
    #[error("Der Server konnte das Konto nicht speichern")]
    Storage,
    #[error("Für diese E-Mail-Adresse gibt es schon ein Konto")]
    EmailTaken,
    #[error("E-Mail-Adresse oder Passwort stimmen nicht")]
    WrongPassword,
    #[error("Zu viele falsche Versuche, das Konto ist für 15 Minuten gesperrt")]
    Locked,
    #[error("Bitte eine gültige E-Mail-Adresse angeben")]
    InvalidEmail,
    #[error("Die Verbindung zum Server ist nicht verschlüsselt, Anmelden ist so nicht möglich (Update nötig)")]
    Unencrypted,
}

impl AccountOp {
    /// Requests that carry values derived from a password or recovery code;
    /// the server takes them only over an encrypted connection.
    pub fn carries_secrets(&self) -> bool {
        matches!(self, Self::Register { .. } | Self::SetLogin { .. } | Self::Login { .. } | Self::Recover { .. })
    }
}

/// An address in canonical form (trimmed, lowercase), if it looks like one.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && !domain.contains('@');
    ok.then_some(email)
}

fn signed_payload(nonce: &Nonce, op: &AccountOp) -> Vec<u8> {
    let mut payload = b"ctxremote/account/v1:".to_vec();
    payload.extend_from_slice(nonce);
    payload.extend(postcard::to_stdvec(op).expect("ops serialize"));
    payload
}

/// Signs `op` for the connection with `nonce`. Separate from registration
/// and alias payloads, so no signature can be replayed as another kind.
pub fn sign(key: &SigningKey, nonce: &Nonce, op: &AccountOp) -> DeviceAuth {
    DeviceAuth {
        public_key: key.verifying_key().to_bytes(),
        signature: key.sign(&signed_payload(nonce, op)).to_bytes().to_vec(),
    }
}

pub fn verify(auth: &DeviceAuth, nonce: &Nonce, op: &AccountOp) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(&auth.public_key) else { return false };
    let Ok(signature) = Signature::from_slice(&auth.signature) else { return false };
    key.verify(&signed_payload(nonce, op), &signature).is_ok()
}

/// Whether `code_id` could be the first part of a pairing code.
pub fn valid_code_id(code_id: &str) -> bool {
    code_id.len() == CODE_ID_LEN && code_id.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

/// A typed pairing code in canonical form: uppercase, without separators,
/// with the usual misreadings fixed (O → 0, I and L → 1). `None` if it cannot be one.
pub fn normalize_code(code: &str) -> Option<String> {
    let code: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (code.len() == CODE_ID_LEN + CODE_SECRET_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_cover_nonce_and_request() {
        let key = SigningKey::from_bytes(&[4; 32]);
        let auth = sign(&key, &[1; 32], &AccountOp::GetBook);
        assert!(verify(&auth, &[1; 32], &AccountOp::GetBook));
        assert!(!verify(&auth, &[2; 32], &AccountOp::GetBook));
        assert!(!verify(&auth, &[1; 32], &AccountOp::Leave));
        // A registration signature over the same nonce is no account signature.
        let register = DeviceAuth { signature: crate::rendezvous::sign_challenge(&key, &[1; 32]), ..auth };
        assert!(!verify(&register, &[1; 32], &AccountOp::GetBook));
    }

    #[test]
    fn emails() {
        assert_eq!(normalize_email(" Philipp@Example.ORG ").as_deref(), Some("philipp@example.org"));
        for bad in ["", "a@b", "@b.de", "a@.de", "a@b.", "a b@c.de", "a@b@c.de"] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        assert!(Kdf::CURRENT.acceptable());
        assert!(!Kdf { memory_kib: 1024, ..Kdf::CURRENT }.acceptable());
    }

    #[test]
    fn codes() {
        assert_eq!(normalize_code("abcd-efgh-jkmn").as_deref(), Some("ABCDEFGHJKMN"));
        assert_eq!(normalize_code(" O1IL 2345 6789 ").as_deref(), Some("0111234567 89".replace(' ', "").as_str()));
        assert_eq!(normalize_code("ABCD-EFGH-JKM"), None);
        assert_eq!(normalize_code("ABCD-EFGH-JKMU"), None);
        assert!(valid_code_id("7K2P"));
        assert!(!valid_code_id("7K2"));
        assert!(!valid_code_id("7k2p"));
    }
}
