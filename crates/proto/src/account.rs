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
/// Most accounts in one team.
pub const MAX_TEAM: usize = 50;

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
    /// Asks the account's other online devices to send Wake-on-LAN packets
    /// for these MACs into their networks. Answered with `Woken`.
    Wake { macs: Vec<String> },

    // Teams (see docs/ACCOUNTS.md, "Teams"). The account acts, whichever of
    // its devices (or its web login) sends them.
    /// Creates a team with this account as its owner, at epoch 1.
    CreateTeam { key: MemberKey, blob: Vec<u8> },
    /// The team of this account. Answered with `Team`.
    GetTeam,
    /// Starts an invitation (owner, admins): like `OfferPairing`, with the
    /// team key sealed for the code, for a newcomer with `role` (not owner).
    OfferInvite { code_id: String, salt: [u8; 16], verifier: [u8; 32], sealed: Vec<u8>, role: TeamRole },
    /// The salt of an invitation.
    InviteSalt { code_id: String },
    /// Joins the team of the invitation with this account's exchange key.
    /// Answered with `JoinedTeam`; the member stays unconfirmed until `ConfirmMember`.
    JoinTeam { code_id: String, proof: [u8; 32], public_key: [u8; 32] },
    /// Right after joining: the member's tag and the team key sealed for itself.
    ConfirmMember { epoch: u32, key: MemberKey },
    /// Replaces the shared address book (owner, admins), if it is still at
    /// revision `base` and the team at `epoch`.
    PutTeamBook { epoch: u32, base: u64, blob: Vec<u8> },
    /// Changes a member's role (owner, admins). Nobody changes the owner's
    /// role; making someone owner is the owner's handover, it becomes an admin.
    SetRole { member: u64, role: TeamRole },
    /// Removes `remove` (may be empty) and moves the team to a new epoch
    /// (owner, admins); see [`Rotation`].
    Rotate { remove: Vec<u64>, rotation: Rotation },
    /// This account leaves its team (not the owner).
    LeaveTeam,
    /// Ends the team (owner).
    DeleteTeam,
    /// This member's own list of devices it opened to the team, encrypted
    /// with the team key of `epoch`.
    SetTeamAccess { epoch: u32, blob: Vec<u8> },
    /// The hash of the report token of `epoch` (owner, admins), so the server
    /// can tell who may add reports without knowing the team key.
    SetReportCheck { epoch: u32, check: [u8; 32] },
    /// One session report, encrypted with the report key of `epoch`. Any
    /// device with the team's report token may send it, also one in no
    /// account (a service host opened to the team).
    AddReport { team: [u8; 16], epoch: u32, token: [u8; 32], blob: Vec<u8> },
    /// Reports after sequence number `after` (owner, admins), oldest first,
    /// at most [`MAX_REPORTS_PER_REPLY`]. Answered with `Reports`.
    GetReports { after: u64 },
    /// Replaces the team's policy (owner, admins), encrypted with the report
    /// key of `epoch`; empty removes it (see `docs/ACCOUNTS.md`, "Richtlinien").
    SetPolicy { epoch: u32, blob: Vec<u8> },
    /// The team's policy, for any device with the report token of `epoch`,
    /// also one in no account (a service host opened to the team). Answered
    /// with `Policy`.
    GetPolicy { team: [u8; 16], epoch: u32, token: [u8; 32] },
}

/// Largest encrypted session report.
pub const MAX_REPORT: usize = 2048;
/// Reports per `GetReports` answer.
pub const MAX_REPORTS_PER_REPLY: usize = 500;
/// Largest encrypted team policy.
pub const MAX_POLICY: usize = 4096;

/// One stored session report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportEntry {
    pub seq: u64,
    /// Unix seconds when the server got it.
    pub at: u64,
    pub epoch: u32,
    /// The member whose device sent it, as far as the server knows; `None`
    /// for a device in no account of the team (a host opened to the team).
    pub member: Option<u64>,
    pub blob: Vec<u8>,
}

/// A member's exchange key with its tag, and the team key sealed for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberKey {
    /// X25519, derived from the account key.
    pub public_key: [u8; 32],
    /// HMAC with a key derived from the team key over member ID and
    /// `public_key`: the server cannot make one up.
    pub tag: [u8; 32],
    /// The team key, sealed for `public_key`.
    pub sealed: Vec<u8>,
}

/// A new epoch: a new team key, everything encrypted with it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rotation {
    /// One more than the current epoch.
    pub epoch: u32,
    /// The revision of the shared address book this was made from.
    pub base: u64,
    pub blob: Vec<u8>,
    /// Exactly the members that stay, with their new tags and sealed keys.
    pub members: Vec<(u64, MemberKey)>,
    /// The members' access lists, encrypted again (see `SetTeamAccess`).
    pub access: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TeamRole {
    Technician,
    Admin,
    Owner,
}

impl TeamRole {
    /// Changes the address book, invites, removes members, starts epochs.
    pub fn manages(self) -> bool {
        self >= Self::Admin
    }
}

/// The team as one member sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamState {
    pub id: [u8; 16],
    pub epoch: u32,
    pub revision: u64,
    /// The shared address book, encrypted with the team key.
    pub blob: Vec<u8>,
    /// This account's member ID.
    pub me: u64,
    /// The team key sealed for this account; `None` until confirmed.
    pub sealed: Option<Vec<u8>>,
    pub members: Vec<TeamMember>,
    /// Someone left: an admin's app should start a new epoch.
    pub stale: bool,
    /// Member ID → its access list (see `SetTeamAccess`).
    pub access: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMember {
    pub id: u64,
    pub role: TeamRole,
    /// Its login address, if it has one.
    pub email: Option<String>,
    /// `None` while unconfirmed.
    pub key: Option<([u8; 32], [u8; 32])>,
    /// Unix seconds of joining.
    pub since: u64,
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
    /// How many devices of the account were asked to wake.
    Woken(u32),
    /// `None`: this account is in no team.
    Team(Option<TeamState>),
    /// After `JoinTeam`: the team key sealed for the code, and the team.
    JoinedTeam { sealed: Vec<u8>, team: TeamState },
    /// After `GetReports`; `more`: there are newer ones still.
    Reports { entries: Vec<ReportEntry>, more: bool },
    /// After `GetPolicy`: the stored policy and the epoch it was encrypted
    /// for; an empty blob if the team has none.
    Policy { epoch: u32, blob: Vec<u8> },
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
    // The web interface recognises these two by their text
    // (web/src/lib/team.ts, `NO_TEAM` and `STALE_KEY`); change both together.
    #[error("Dieses Konto gehört zu keinem Team")]
    NoTeam,
    #[error("Dieses Konto gehört schon zu einem Team")]
    AlreadyInTeam,
    #[error("Dafür fehlt die Rolle im Team")]
    NotAllowed,
    #[error("Der Teamschlüssel wurde inzwischen erneuert, bitte erneut versuchen")]
    StaleKey,
    #[error("Das Team hat schon die größte Zahl an Mitgliedern")]
    TeamFull,
    #[error("Der Inhaber muss das Team erst übergeben oder löschen")]
    OwnerStays,
    #[error("Der Sitzungsbericht wurde abgelehnt (Teamschlüssel erneuert?)")]
    ReportRefused,
}

impl AccountOp {
    /// Requests about the account's team (see `docs/ACCOUNTS.md`, "Teams").
    pub fn is_team(&self) -> bool {
        matches!(
            self,
            Self::CreateTeam { .. }
                | Self::GetTeam
                | Self::OfferInvite { .. }
                | Self::InviteSalt { .. }
                | Self::JoinTeam { .. }
                | Self::ConfirmMember { .. }
                | Self::PutTeamBook { .. }
                | Self::SetRole { .. }
                | Self::Rotate { .. }
                | Self::LeaveTeam
                | Self::DeleteTeam
                | Self::SetTeamAccess { .. }
                | Self::SetReportCheck { .. }
                | Self::GetReports { .. }
                | Self::SetPolicy { .. }
        )
    }

    /// Requests that carry values derived from a password or recovery code
    /// (or the report token); the server takes them only over an encrypted
    /// connection.
    pub fn carries_secrets(&self) -> bool {
        matches!(
            self,
            Self::Register { .. }
                | Self::SetLogin { .. }
                | Self::Login { .. }
                | Self::Recover { .. }
                | Self::AddReport { .. }
                | Self::GetPolicy { .. }
        )
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
