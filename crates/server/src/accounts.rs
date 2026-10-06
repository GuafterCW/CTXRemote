//! Accounts: sets of device keys sharing an encrypted address book (see
//! `ctxremote_proto::account` and `docs/ACCOUNTS.md`).
//!
//! Stored in `accounts.json` next to the registry. Pairings live in memory
//! only; a restart simply ends them.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ctxremote_proto::account::{
    normalize_email, valid_code_id, AccountError, AccountInfo, AccountOp, AccountReply, Kdf, LoginInfo, LoginSetup,
    Member, MAX_BOOK, MAX_DEVICES, PAIRING_SECS,
};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;

use crate::mail::Mail;
use serde::{Deserialize, Serialize};

/// Wrong proofs a pairing survives; then it is gone and needs a new code.
const MAX_TRIES: u32 = 3;
/// Wrong passwords (or recovery codes) per account before it locks for [`LOCKOUT`].
const MAX_FAILURES: u32 = 10;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// Longest encrypted device name.
const MAX_LABEL: usize = 512;
/// How long the link in a confirmation mail works.
const VERIFY_SECS: u64 = 48 * 3600;
/// Shortest pause between two confirmation mails for one account.
const RESEND_PAUSE: Duration = Duration::from_secs(5 * 60);

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    next: u64,
    accounts: BTreeMap<u64, Account>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Account {
    /// Hex-encoded Ed25519 public keys of the member devices.
    keys: Vec<String>,
    revision: u64,
    /// The encrypted address book, hex-encoded; the server cannot read it.
    book: String,
    #[serde(default)]
    login: Option<StoredLogin>,
    /// Public key (hex) → its name, encrypted with the account key (hex).
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

/// A login as stored: hashes of the derived values, never the values.
#[derive(Clone, Serialize, Deserialize)]
struct StoredLogin {
    email: String,
    salt: String,
    kdf: Kdf,
    /// SHA-256 of the value derived from the password (hex).
    auth_hash: String,
    /// The account key sealed for the password (hex).
    wrapped: String,
    recovery_hash: String,
    recovery_wrapped: String,
    #[serde(default)]
    verified: bool,
    /// The pending confirmation: SHA-256 of the link's token (hex) and when it expires (Unix seconds).
    #[serde(default)]
    verify: Option<(String, u64)>,
    /// When the address was set (Unix seconds). Unconfirmed, it only holds
    /// the address for [`VERIFY_SECS`]; then anyone may register with it.
    #[serde(default = "unix_now")]
    since: u64,
}

impl StoredLogin {
    fn from_setup(setup: LoginSetup, verified: bool) -> Result<Self, AccountError> {
        let email = normalize_email(&setup.email).ok_or(AccountError::InvalidEmail)?;
        if !setup.kdf.acceptable() || setup.wrapped.len() > 256 || setup.recovery_wrapped.len() > 256 {
            return Err(AccountError::InvalidEmail);
        }
        Ok(Self {
            email,
            salt: hex::encode(setup.salt),
            kdf: setup.kdf,
            auth_hash: hex::encode(Sha256::digest(setup.auth)),
            wrapped: hex::encode(setup.wrapped),
            recovery_hash: hex::encode(Sha256::digest(setup.recovery_auth)),
            recovery_wrapped: hex::encode(setup.recovery_wrapped),
            verified,
            verify: None,
            since: unix_now(),
        })
    }

    /// Starts a confirmation of the address; returns the token for the link.
    fn new_verification(&mut self) -> String {
        use base64::Engine;
        let token: [u8; 32] = rand::random();
        let expires = unix_now() + VERIFY_SECS;
        self.verify = Some((hex::encode(Sha256::digest(token)), expires));
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
    }
}

struct Pairing {
    account: u64,
    salt: [u8; 16],
    verifier: [u8; 32],
    sealed: Vec<u8>,
    expires: Instant,
    tries: u32,
}

pub struct Accounts {
    path: PathBuf,
    stored: Stored,
    /// Public key (hex) → account, derived from `stored`.
    by_key: HashMap<String, u64>,
    pairings: HashMap<String, Pairing>,
    /// Address → account, derived from `stored`.
    by_email: HashMap<String, u64>,
    /// Wrong passwords per account: count and the first one's time.
    failures: HashMap<u64, (u32, Instant)>,
    /// Secret for made-up salts of unknown addresses.
    pepper: [u8; 32],
    /// Where mails go; `None` in tests and tools.
    mail: Option<UnboundedSender<Mail>>,
    /// When each account last got a confirmation mail on request.
    resent: HashMap<u64, Instant>,
}

impl Accounts {
    pub fn open(path: PathBuf, pepper: [u8; 32]) -> Result<Self> {
        let stored: Stored = match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).with_context(|| format!("{} ist beschädigt", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(e) => return Err(e.into()),
        };
        let by_key = stored
            .accounts
            .iter()
            .flat_map(|(id, account)| account.keys.iter().map(move |key| (key.clone(), *id)))
            .collect();
        let by_email = stored
            .accounts
            .iter()
            .filter_map(|(id, account)| Some((account.login.as_ref()?.email.clone(), *id)))
            .collect();
        Ok(Self {
            path,
            stored,
            by_key,
            pairings: HashMap::new(),
            by_email,
            failures: HashMap::new(),
            pepper,
            mail: None,
            resent: HashMap::new(),
        })
    }

    /// Sends account mails through `mail` (see `crate::mail`).
    pub fn with_mail(mut self, mail: UnboundedSender<Mail>) -> Self {
        self.mail = Some(mail);
        self
    }

    fn emit(&self, mail: Mail) {
        if let Some(tx) = &self.mail {
            let _ = tx.send(mail);
        }
    }

    /// The address of `account`, if it has a login.
    fn email(&self, account: u64) -> Option<String> {
        Some(self.stored.accounts.get(&account)?.login.as_ref()?.email.clone())
    }

    /// Confirms the address whose mail carried `token`.
    pub fn verify_email(&mut self, token: &str) -> Result<(), AccountError> {
        use base64::Engine;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.trim()).map_err(|_| AccountError::UnknownCode)?;
        let hash = hex::encode(Sha256::digest(raw));
        let now = unix_now();
        let id = self
            .stored
            .accounts
            .iter()
            .find(|(_, a)| a.login.as_ref().and_then(|l| l.verify.as_ref()).is_some_and(|(h, exp)| *h == hash && *exp > now))
            .map(|(id, _)| *id)
            .ok_or(AccountError::UnknownCode)?;
        let before = self.snapshot();
        let login = self.stored.accounts.get_mut(&id).and_then(|a| a.login.as_mut()).expect("found above");
        login.verified = true;
        login.verify = None;
        self.commit(before)
    }

    /// Who else holds `email`, if they may lose it: an address nobody
    /// confirmed within [`VERIFY_SECS`] goes to whoever registers it next, so
    /// a stranger cannot keep someone's address from them by signing up first.
    fn stale_holder(&self, email: &str, me: Option<u64>) -> Result<Option<u64>, AccountError> {
        let Some(&owner) = self.by_email.get(email) else { return Ok(None) };
        if Some(owner) == me {
            return Ok(None);
        }
        let login = self.stored.accounts.get(&owner).and_then(|a| a.login.as_ref());
        match login {
            Some(l) if l.verified || unix_now() < l.since + VERIFY_SECS => Err(AccountError::EmailTaken),
            _ => Ok(Some(owner)),
        }
    }

    /// Takes the unconfirmed login away from `owner` (see [`Self::stale_holder`]).
    /// Its devices stay in the account.
    fn release(&mut self, owner: Option<u64>) {
        let Some(owner) = owner else { return };
        if let Some(login) = self.stored.accounts.get_mut(&owner).and_then(|a| a.login.take()) {
            self.by_email.remove(&login.email);
            tracing::info!(account = owner, "Unbestätigte Adresse freigegeben");
        }
    }

    /// Sends the confirmation mail again, at most every few minutes.
    pub fn resend_verification(&mut self, account: u64) -> Result<(), AccountError> {
        if self.resent.get(&account).is_some_and(|at| at.elapsed() < RESEND_PAUSE) {
            return Err(AccountError::RateLimited);
        }
        let before = self.snapshot();
        let login = self.stored.accounts.get_mut(&account).and_then(|a| a.login.as_mut()).ok_or(AccountError::NotLinked)?;
        if login.verified {
            return Ok(());
        }
        let token = login.new_verification();
        let to = login.email.clone();
        self.commit(before)?;
        self.resent.insert(account, Instant::now());
        self.emit(Mail::Verify { to, token });
        Ok(())
    }

    /// Whether both keys are members of one account (for access without a
    /// password; the host proves nothing here, so this reveals only whether
    /// two keys the asker already knows belong together).
    pub fn same_account(&self, a: &[u8; 32], b: &[u8; 32]) -> bool {
        match (self.by_key.get(&hex::encode(a)), self.by_key.get(&hex::encode(b))) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }

    /// Carries out a request whose signature the caller has checked.
    /// The device keys of `public_key`'s account, if it has one.
    pub fn member_keys(&self, public_key: [u8; 32]) -> Option<Vec<[u8; 32]>> {
        let account = self.by_key.get(&hex::encode(public_key))?;
        let keys = &self.stored.accounts.get(account)?.keys;
        Some(keys.iter().filter_map(|k| hex::decode(k).ok()?.try_into().ok()).collect())
    }

    pub fn handle(&mut self, public_key: [u8; 32], op: AccountOp) -> Result<AccountReply, AccountError> {
        let key = hex::encode(public_key);
        let member = self.by_key.get(&key).copied();
        self.run(key, member, op)
    }

    /// A request from a signed-in browser (the web interface): it acts for
    /// `account` without a device key, so only what needs no key is allowed.
    pub fn handle_web(&mut self, account: u64, op: AccountOp) -> Result<AccountReply, AccountError> {
        if !self.stored.accounts.contains_key(&account) {
            return Err(AccountError::NotLinked);
        }
        match op {
            AccountOp::Status
            | AccountOp::GetBook
            | AccountOp::PutBook { .. }
            | AccountOp::Devices
            | AccountOp::RemoveDevice { .. }
            | AccountOp::SetLogin { .. }
            | AccountOp::LoginStatus
            | AccountOp::OfferPairing { .. } => self.run(String::new(), Some(account), op),
            _ => Err(AccountError::NotLinked),
        }
    }

    /// Web sign-up: an account with a login and no devices yet.
    pub fn register_web(&mut self, login: LoginSetup) -> Result<u64, AccountError> {
        let mut login = StoredLogin::from_setup(login, false)?;
        let stale = self.stale_holder(&login.email, None)?;
        let token = login.new_verification();
        let verify = Mail::Verify { to: login.email.clone(), token };
        let id = self.stored.next.max(1);
        let before = self.snapshot();
        self.release(stale);
        self.stored.next = id + 1;
        self.by_email.insert(login.email.clone(), id);
        self.stored.accounts.insert(id, Account { login: Some(login), ..Default::default() });
        self.commit(before)?;
        self.emit(verify);
        Ok(id)
    }

    /// Web sign-in: checks the value derived from the password (or the
    /// recovery code) and returns the account with the matching sealed key.
    pub fn login_web(&mut self, email: &str, value: &[u8; 32], recovery: bool) -> Result<(u64, Vec<u8>), AccountError> {
        let (id, wrapped) = self.check_login(email, value, recovery)?;
        if let Some(to) = self.email(id) {
            self.emit(if recovery { Mail::RecoveryUsed { to } } else { Mail::NewSignIn { to, how: "ein Browser im Webinterface" } });
        }
        Ok((id, wrapped))
    }

    /// Checks the value derived from `account`'s current password.
    pub fn confirm_password(&mut self, account: u64, value: &[u8; 32]) -> Result<(), AccountError> {
        let email = self.email(account).ok_or(AccountError::NotLinked)?;
        let (id, _) = self.check_login(&email, value, false)?;
        if id != account {
            return Err(AccountError::WrongPassword);
        }
        Ok(())
    }

    /// Deletes `account` for good, after the password once more: devices,
    /// login, list. Its devices find out on their next sync (`NotLinked`).
    pub fn delete_web(&mut self, account: u64, value: &[u8; 32]) -> Result<(), AccountError> {
        self.confirm_password(account, value)?;
        let (id, email) = (account, self.email(account).ok_or(AccountError::NotLinked)?);
        let before = self.snapshot();
        let removed = self.stored.accounts.remove(&id).expect("checked above");
        for key in &removed.keys {
            self.by_key.remove(key);
        }
        self.by_email.retain(|_, a| *a != id);
        self.commit(before)?;
        self.pairings.retain(|_, p| p.account != id);
        self.failures.remove(&id);
        self.resent.remove(&id);
        self.emit(Mail::AccountDeleted { to: email });
        Ok(())
    }

    fn run(&mut self, key: String, member: Option<u64>, op: AccountOp) -> Result<AccountReply, AccountError> {
        let now = Instant::now();
        self.pairings.retain(|_, p| p.expires > now);

        match op {
            AccountOp::Status => Ok(AccountReply::Status(member.map(|id| self.info(id)))),
            AccountOp::Create => {
                if member.is_some() {
                    return Err(AccountError::AlreadyLinked);
                }
                let id = self.stored.next.max(1);
                let before = self.snapshot();
                self.stored.next = id + 1;
                self.stored.accounts.insert(id, Account { keys: vec![key.clone()], ..Default::default() });
                self.by_key.insert(key, id);
                self.commit(before)?;
                Ok(AccountReply::Created(self.info(id)))
            }
            AccountOp::OfferPairing { code_id, salt, verifier, sealed } => {
                let account = member.ok_or(AccountError::NotLinked)?;
                if !valid_code_id(&code_id) || sealed.len() > 256 {
                    return Err(AccountError::UnknownCode);
                }
                if self.stored.accounts[&account].keys.len() >= MAX_DEVICES {
                    return Err(AccountError::TooManyDevices);
                }
                // One open pairing per account; a code in use elsewhere makes the device pick another.
                if self.pairings.get(&code_id).is_some_and(|p| p.account != account) {
                    return Err(AccountError::Conflict);
                }
                self.pairings.retain(|_, p| p.account != account);
                let expires = now + Duration::from_secs(PAIRING_SECS.into());
                self.pairings.insert(code_id, Pairing { account, salt, verifier, sealed, expires, tries: 0 });
                Ok(AccountReply::PairingOffered)
            }
            AccountOp::PairingSalt { code_id } => {
                self.pairings.get(&code_id).map(|p| AccountReply::Salt(p.salt)).ok_or(AccountError::UnknownCode)
            }
            AccountOp::JoinPairing { code_id, proof } => {
                let pairing = self.pairings.get_mut(&code_id).ok_or(AccountError::UnknownCode)?;
                if !constant_time_eq(&pairing.verifier, &proof) {
                    pairing.tries += 1;
                    if pairing.tries >= MAX_TRIES {
                        self.pairings.remove(&code_id);
                    }
                    return Err(AccountError::WrongCode);
                }
                let account = pairing.account;
                match member {
                    Some(own) if own == account => {}
                    Some(_) => return Err(AccountError::AlreadyLinked),
                    None => {
                        if self.stored.accounts[&account].keys.len() >= MAX_DEVICES {
                            return Err(AccountError::TooManyDevices);
                        }
                        let before = self.snapshot();
                        self.stored.accounts.get_mut(&account).expect("pairings point to accounts").keys.push(key.clone());
                        self.by_key.insert(key, account);
                        self.commit(before)?;
                    }
                }
                let pairing = self.pairings.remove(&code_id).expect("looked up above");
                if let (None, Some(to)) = (member, self.email(account)) {
                    self.emit(Mail::NewSignIn { to, how: "ein neues Gerät mit einem Kopplungscode" });
                }
                Ok(AccountReply::Joined { info: self.info(account), sealed: pairing.sealed })
            }
            AccountOp::GetBook => {
                let account = &self.stored.accounts[&member.ok_or(AccountError::NotLinked)?];
                let blob = hex::decode(&account.book).unwrap_or_default();
                Ok(AccountReply::Book { revision: account.revision, blob })
            }
            AccountOp::PutBook { base, blob } => {
                let id = member.ok_or(AccountError::NotLinked)?;
                if blob.len() > MAX_BOOK {
                    return Err(AccountError::TooLarge);
                }
                if self.stored.accounts[&id].revision != base {
                    return Err(AccountError::Conflict);
                }
                let before = self.snapshot();
                let account = self.stored.accounts.get_mut(&id).expect("member of it");
                account.revision += 1;
                account.book = hex::encode(&blob);
                let revision = account.revision;
                self.commit(before)?;
                Ok(AccountReply::Stored { revision })
            }
            AccountOp::Prelogin { email } => {
                let email = normalize_email(&email).ok_or(AccountError::InvalidEmail)?;
                Ok(match self.by_email.get(&email).and_then(|id| self.stored.accounts[id].login.as_ref()) {
                    Some(login) => AccountReply::Prelogin { salt: unhex16(&login.salt), kdf: login.kdf },
                    None => AccountReply::Prelogin { salt: self.fake_salt(&email), kdf: Kdf::CURRENT },
                })
            }
            AccountOp::Register { login } => {
                if member.is_some() {
                    return Err(AccountError::AlreadyLinked);
                }
                let mut login = StoredLogin::from_setup(login, false)?;
                let stale = self.stale_holder(&login.email, None)?;
                let token = login.new_verification();
                let verify = Mail::Verify { to: login.email.clone(), token };
                let id = self.stored.next.max(1);
                let before = self.snapshot();
                self.release(stale);
                self.stored.next = id + 1;
                let wrapped = hex::decode(&login.wrapped).unwrap_or_default();
                self.by_email.insert(login.email.clone(), id);
                self.stored.accounts.insert(id, Account { keys: vec![key.clone()], login: Some(login), ..Default::default() });
                self.by_key.insert(key, id);
                self.commit(before)?;
                self.emit(verify);
                Ok(AccountReply::LoggedIn { info: self.info(id), wrapped })
            }
            AccountOp::SetLogin { login } => {
                let id = member.ok_or(AccountError::NotLinked)?;
                let old = self.stored.accounts[&id].login.clone();
                let same_address = old.as_ref().is_some_and(|o| normalize_email(&login.email).as_deref() == Some(o.email.as_str()));
                let mut login = StoredLogin::from_setup(login, same_address && old.as_ref().is_some_and(|o| o.verified))?;
                let stale = self.stale_holder(&login.email, Some(id))?;
                let mut mails = Vec::new();
                if same_address {
                    // A pending confirmation stays valid with a new password,
                    // and so does the claim on the address.
                    login.verify = old.as_ref().and_then(|o| o.verify.clone());
                    login.since = old.as_ref().map_or(login.since, |o| o.since);
                    mails.push(Mail::PasswordChanged { to: login.email.clone() });
                } else {
                    let token = login.new_verification();
                    mails.push(Mail::Verify { to: login.email.clone(), token });
                    if let Some(old) = &old {
                        mails.push(Mail::AddressChanged { to: old.email.clone(), new: login.email.clone() });
                    }
                }
                let before = self.snapshot();
                self.release(stale);
                if let Some(old) = &old {
                    self.by_email.remove(&old.email);
                }
                self.by_email.insert(login.email.clone(), id);
                self.stored.accounts.get_mut(&id).expect("member of it").login = Some(login);
                self.commit(before)?;
                self.failures.remove(&id);
                for mail in mails {
                    self.emit(mail);
                }
                Ok(AccountReply::Done)
            }
            AccountOp::Login { email, auth } => self.join_with(member, key, &email, &auth, false),
            AccountOp::Recover { email, recovery_auth } => self.join_with(member, key, &email, &recovery_auth, true),
            AccountOp::LoginStatus => {
                let id = member.ok_or(AccountError::NotLinked)?;
                let login = self.stored.accounts[&id].login.as_ref();
                Ok(AccountReply::LoginStatus(login.map(|l| LoginInfo { email: l.email.clone(), verified: l.verified })))
            }
            AccountOp::Devices => {
                let account = &self.stored.accounts[&member.ok_or(AccountError::NotLinked)?];
                let members = account
                    .keys
                    .iter()
                    .filter_map(|k| {
                        let public_key: [u8; 32] = hex::decode(k).ok()?.try_into().ok()?;
                        let label = account.labels.get(k).and_then(|l| hex::decode(l).ok()).unwrap_or_default();
                        Some(Member { public_key, device: None, online: false, label })
                    })
                    .collect();
                Ok(AccountReply::Devices(members))
            }
            AccountOp::RemoveDevice { public_key } => {
                let id = member.ok_or(AccountError::NotLinked)?;
                let target = hex::encode(public_key);
                if self.by_key.get(&target) != Some(&id) {
                    return Err(AccountError::NotLinked);
                }
                let before = self.snapshot();
                let account = self.stored.accounts.get_mut(&id).expect("member of it");
                account.keys.retain(|k| *k != target);
                account.labels.remove(&target);
                self.by_key.remove(&target);
                self.commit(before)?;
                Ok(AccountReply::Done)
            }
            AccountOp::SetLabel { label } => {
                let id = member.ok_or(AccountError::NotLinked)?;
                if label.len() > MAX_LABEL {
                    return Err(AccountError::TooLarge);
                }
                let before = self.snapshot();
                let account = self.stored.accounts.get_mut(&id).expect("member of it");
                if label.is_empty() {
                    account.labels.remove(&key);
                } else {
                    account.labels.insert(key, hex::encode(label));
                }
                self.commit(before)?;
                Ok(AccountReply::Done)
            }
            // The server itself passes it on to the devices (`Server::wake_for`).
            AccountOp::Wake { .. } => Ok(AccountReply::Woken(0)),
            AccountOp::Leave => {
                let id = member.ok_or(AccountError::NotLinked)?;
                let before = self.snapshot();
                let account = self.stored.accounts.get_mut(&id).expect("member of it");
                account.keys.retain(|k| *k != key);
                account.labels.remove(&key);
                // Without devices and without a login nobody could ever reach it again.
                if account.keys.is_empty() && account.login.is_none() {
                    self.stored.accounts.remove(&id);
                    self.pairings.retain(|_, p| p.account != id);
                }
                self.by_key.remove(&key);
                self.commit(before)?;
                Ok(AccountReply::Left)
            }
        }
    }

    /// `Login` and `Recover`: checks the derived value and adds this device.
    fn join_with(
        &mut self,
        member: Option<u64>,
        key: String,
        email: &str,
        value: &[u8; 32],
        recovery: bool,
    ) -> Result<AccountReply, AccountError> {
        let (id, wrapped) = self.check_login(email, value, recovery)?;
        if member.is_some_and(|m| m != id) {
            return Err(AccountError::AlreadyLinked);
        }
        if member.is_none() {
            if self.stored.accounts[&id].keys.len() >= MAX_DEVICES {
                return Err(AccountError::TooManyDevices);
            }
            let before = self.snapshot();
            self.stored.accounts.get_mut(&id).expect("exists").keys.push(key.clone());
            self.by_key.insert(key, id);
            self.commit(before)?;
            if let (false, Some(to)) = (recovery, self.email(id)) {
                self.emit(Mail::NewSignIn { to, how: "ein neues Gerät mit Ihrem Passwort" });
            }
        }
        if let (true, Some(to)) = (recovery, self.email(id)) {
            self.emit(Mail::RecoveryUsed { to });
        }
        Ok(AccountReply::LoggedIn { info: self.info(id), wrapped })
    }

    /// Checks a password-derived (or recovery) value; counts failures and
    /// locks the account after too many. Returns the account and its sealed key.
    fn check_login(&mut self, email: &str, value: &[u8; 32], recovery: bool) -> Result<(u64, Vec<u8>), AccountError> {
        let email = normalize_email(email).ok_or(AccountError::WrongPassword)?;
        let id = *self.by_email.get(&email).ok_or(AccountError::WrongPassword)?;
        let now = Instant::now();
        let failures = self.failures.entry(id).or_insert((0, now));
        if now.duration_since(failures.1) > LOCKOUT {
            *failures = (0, now);
        }
        // The recovery code (125 bits) cannot be guessed; checking it even
        // while locked keeps the owner from being locked out by a stranger
        // who types wrong passwords on purpose.
        if failures.0 >= MAX_FAILURES && !recovery {
            return Err(AccountError::Locked);
        }
        let login = self.stored.accounts[&id].login.clone().expect("indexed by its login");
        let (expected, wrapped) = if recovery {
            (&login.recovery_hash, &login.recovery_wrapped)
        } else {
            (&login.auth_hash, &login.wrapped)
        };
        let given: [u8; 32] = Sha256::digest(value).into();
        let expected: [u8; 32] = hex::decode(expected).ok().and_then(|h| h.try_into().ok()).unwrap_or([0xff; 32]);
        if !constant_time_eq(&given, &expected) {
            if !recovery {
                failures.0 += 1;
            }
            return Err(AccountError::WrongPassword);
        }
        self.failures.remove(&id);
        Ok((id, hex::decode(wrapped).unwrap_or_default()))
    }

    /// Stable per address, so asking twice cannot tell a made-up salt apart.
    fn fake_salt(&self, email: &str) -> [u8; 16] {
        let mut salt = [0u8; 16];
        Hkdf::<Sha256>::new(Some(&self.pepper), email.as_bytes())
            .expand(b"ctxremote/fake-salt/v1", &mut salt)
            .expect("16 bytes is a valid HKDF length");
        salt
    }

    fn info(&self, id: u64) -> AccountInfo {
        AccountInfo { devices: self.stored.accounts.get(&id).map_or(0, |a| a.keys.len() as u32) }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            stored: Stored { next: self.stored.next, accounts: self.stored.accounts.clone() },
            by_key: self.by_key.clone(),
            by_email: self.by_email.clone(),
        }
    }

    /// Saves; on failure memory goes back to `before`, so it never disagrees with disk.
    fn commit(&mut self, before: Snapshot) -> Result<(), AccountError> {
        if let Err(e) = self.save() {
            tracing::warn!("Konten nicht gespeichert: {e:#}");
            (self.stored, self.by_key, self.by_email) = (before.stored, before.by_key, before.by_email);
            return Err(AccountError::Storage);
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(&self.stored)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

struct Snapshot {
    stored: Stored,
    by_key: HashMap<String, u64>,
    by_email: HashMap<String, u64>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn unhex16(text: &str) -> [u8; 16] {
    hex::decode(text).ok().and_then(|b| b.try_into().ok()).unwrap_or([0; 16])
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("ctxremote-accounts-{}.json", rand::random::<u32>()))
    }

    fn offer(code_id: &str) -> AccountOp {
        AccountOp::OfferPairing { code_id: code_id.into(), salt: [5; 16], verifier: [7; 32], sealed: vec![1, 2, 3] }
    }

    #[test]
    fn create_pair_sync_leave() {
        let path = temp();
        let mut accounts = Accounts::open(path.clone(), [0; 32]).unwrap();
        let (a, b, c) = ([1; 32], [2; 32], [3; 32]);

        assert_eq!(accounts.handle(a, AccountOp::Status), Ok(AccountReply::Status(None)));
        assert_eq!(accounts.handle(a, AccountOp::GetBook), Err(AccountError::NotLinked));
        assert!(matches!(accounts.handle(a, AccountOp::Create), Ok(AccountReply::Created(AccountInfo { devices: 1 }))));
        assert_eq!(accounts.handle(a, AccountOp::Create), Err(AccountError::AlreadyLinked));

        // Only members offer pairings.
        assert_eq!(accounts.handle(b, offer("7K2P")), Err(AccountError::NotLinked));
        assert_eq!(accounts.handle(a, offer("7K2P")), Ok(AccountReply::PairingOffered));
        assert_eq!(accounts.handle(b, AccountOp::PairingSalt { code_id: "7K2P".into() }), Ok(AccountReply::Salt([5; 16])));
        assert_eq!(
            accounts.handle(b, AccountOp::JoinPairing { code_id: "7K2P".into(), proof: [0; 32] }),
            Err(AccountError::WrongCode)
        );
        let joined = accounts.handle(b, AccountOp::JoinPairing { code_id: "7K2P".into(), proof: [7; 32] });
        assert_eq!(joined, Ok(AccountReply::Joined { info: AccountInfo { devices: 2 }, sealed: vec![1, 2, 3] }));
        // The code is spent.
        assert_eq!(
            accounts.handle(c, AccountOp::JoinPairing { code_id: "7K2P".into(), proof: [7; 32] }),
            Err(AccountError::UnknownCode)
        );

        // Both see one book; a stale revision is refused.
        assert_eq!(accounts.handle(a, AccountOp::PutBook { base: 0, blob: vec![9; 10] }), Ok(AccountReply::Stored { revision: 1 }));
        assert_eq!(accounts.handle(b, AccountOp::PutBook { base: 0, blob: vec![8] }), Err(AccountError::Conflict));
        assert_eq!(accounts.handle(b, AccountOp::GetBook), Ok(AccountReply::Book { revision: 1, blob: vec![9; 10] }));
        assert_eq!(
            accounts.handle(b, AccountOp::PutBook { base: 1, blob: vec![0; MAX_BOOK + 1] }),
            Err(AccountError::TooLarge)
        );

        // Survives a restart.
        let mut reopened = Accounts::open(path.clone(), [0; 32]).unwrap();
        assert_eq!(reopened.handle(b, AccountOp::Status), Ok(AccountReply::Status(Some(AccountInfo { devices: 2 }))));

        // The last device to leave deletes the account.
        assert_eq!(reopened.handle(a, AccountOp::Leave), Ok(AccountReply::Left));
        assert_eq!(reopened.handle(b, AccountOp::Status), Ok(AccountReply::Status(Some(AccountInfo { devices: 1 }))));
        assert_eq!(reopened.handle(b, AccountOp::Leave), Ok(AccountReply::Left));
        assert!(reopened.stored.accounts.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    fn setup(email: &str, auth: u8) -> LoginSetup {
        LoginSetup {
            email: email.into(),
            salt: [3; 16],
            kdf: Kdf::CURRENT,
            auth: [auth; 32],
            wrapped: vec![auth; 40],
            recovery_auth: [auth + 100; 32],
            recovery_wrapped: vec![auth + 100; 40],
        }
    }

    #[test]
    fn logins() {
        let path = temp();
        let mut accounts = Accounts::open(path.clone(), [9; 32]).unwrap();
        let (a, b, c) = ([1; 32], [2; 32], [3; 32]);

        // Unknown addresses get a stable made-up salt.
        let fake = accounts.handle(c, AccountOp::Prelogin { email: "x@y.de".into() }).unwrap();
        assert_eq!(fake, accounts.handle(c, AccountOp::Prelogin { email: " X@Y.de".into() }).unwrap());
        assert_ne!(fake, AccountReply::Prelogin { salt: [3; 16], kdf: Kdf::CURRENT });

        let reply = accounts.handle(a, AccountOp::Register { login: setup("Philipp@Example.org", 1) }).unwrap();
        assert_eq!(reply, AccountReply::LoggedIn { info: AccountInfo { devices: 1 }, wrapped: vec![1; 40] });
        assert_eq!(
            accounts.handle(b, AccountOp::Register { login: setup("philipp@example.org", 2) }),
            Err(AccountError::EmailTaken)
        );
        assert_eq!(
            accounts.handle(c, AccountOp::Prelogin { email: "philipp@example.org".into() }),
            Ok(AccountReply::Prelogin { salt: [3; 16], kdf: Kdf::CURRENT })
        );

        // A wrong password, then the right one adds the device.
        let wrong = AccountOp::Login { email: "philipp@example.org".into(), auth: [9; 32] };
        assert_eq!(accounts.handle(b, wrong), Err(AccountError::WrongPassword));
        let login = AccountOp::Login { email: "philipp@example.org".into(), auth: [1; 32] };
        assert!(matches!(accounts.handle(b, login), Ok(AccountReply::LoggedIn { info: AccountInfo { devices: 2 }, .. })));

        // Recovery returns the other sealed copy.
        let recover = AccountOp::Recover { email: "philipp@example.org".into(), recovery_auth: [101; 32] };
        assert_eq!(
            accounts.handle(c, recover),
            Ok(AccountReply::LoggedIn { info: AccountInfo { devices: 3 }, wrapped: vec![101; 40] })
        );
        // A new password replaces the old one.
        assert_eq!(accounts.handle(c, AccountOp::SetLogin { login: setup("philipp@example.org", 5) }), Ok(AccountReply::Done));
        let old = AccountOp::Login { email: "philipp@example.org".into(), auth: [1; 32] };
        assert_eq!(accounts.handle([4; 32], old), Err(AccountError::WrongPassword));

        // Devices, names and removal.
        accounts.handle(a, AccountOp::SetLabel { label: vec![7; 20] }).unwrap();
        let Ok(AccountReply::Devices(members)) = accounts.handle(b, AccountOp::Devices) else { panic!() };
        assert_eq!(members.len(), 3);
        assert_eq!(members.iter().find(|m| m.public_key == a).unwrap().label, vec![7; 20]);
        assert_eq!(accounts.handle(b, AccountOp::RemoveDevice { public_key: c }), Ok(AccountReply::Done));
        assert_eq!(accounts.handle(c, AccountOp::Status), Ok(AccountReply::Status(None)));

        // Survives a restart; an account with a login outlives its devices.
        let mut reopened = Accounts::open(path.clone(), [9; 32]).unwrap();
        reopened.handle(a, AccountOp::Leave).unwrap();
        reopened.handle(b, AccountOp::Leave).unwrap();
        let back = AccountOp::Login { email: "philipp@example.org".into(), auth: [5; 32] };
        assert!(matches!(reopened.handle(a, back), Ok(AccountReply::LoggedIn { info: AccountInfo { devices: 1 }, .. })));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn web_sessions_act_without_a_key() {
        let mut accounts = Accounts::open(temp(), [0; 32]).unwrap();
        let id = accounts.register_web(setup("web@b.de", 1)).unwrap();
        assert_eq!(accounts.register_web(setup("WEB@b.de", 2)), Err(AccountError::EmailTaken));
        assert_eq!(accounts.login_web("web@b.de", &[1; 32], false), Ok((id, vec![1; 40])));
        assert_eq!(accounts.login_web("web@b.de", &[2; 32], false), Err(AccountError::WrongPassword));
        assert_eq!(accounts.login_web("web@b.de", &[101; 32], true), Ok((id, vec![101; 40])));
        assert_eq!(accounts.handle_web(id, AccountOp::PutBook { base: 0, blob: vec![5] }), Ok(AccountReply::Stored { revision: 1 }));
        assert_eq!(accounts.handle_web(id, AccountOp::Devices), Ok(AccountReply::Devices(vec![])));
        // Things that need a device key are not for browsers.
        assert_eq!(accounts.handle_web(id, AccountOp::Leave), Err(AccountError::NotLinked));
        assert_eq!(accounts.handle_web(id, AccountOp::SetLabel { label: vec![1] }), Err(AccountError::NotLinked));
        // A device logs in to the account made on the web and sees the same book.
        let login = AccountOp::Login { email: "web@b.de".into(), auth: [1; 32] };
        assert!(accounts.handle([1; 32], login).is_ok());
        assert_eq!(accounts.handle([1; 32], AccountOp::GetBook), Ok(AccountReply::Book { revision: 1, blob: vec![5] }));
    }

    #[test]
    fn mails_and_confirmation() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut accounts = Accounts::open(temp(), [0; 32]).unwrap().with_mail(tx);
        let mut next = || rx.try_recv().ok();

        accounts.handle([1; 32], AccountOp::Register { login: setup("a@b.de", 1) }).unwrap();
        let Some(Mail::Verify { to, token }) = next() else { panic!("Bestätigungsmail") };
        assert_eq!(to, "a@b.de");
        assert_eq!(accounts.verify_email("falsch"), Err(AccountError::UnknownCode));
        accounts.verify_email(&token).unwrap();
        assert_eq!(accounts.verify_email(&token), Err(AccountError::UnknownCode), "einmal gültig");
        assert_eq!(
            accounts.handle([1; 32], AccountOp::LoginStatus),
            Ok(AccountReply::LoginStatus(Some(LoginInfo { email: "a@b.de".into(), verified: true })))
        );

        // A new device, a new password, a new address.
        accounts.handle([2; 32], AccountOp::Login { email: "a@b.de".into(), auth: [1; 32] }).unwrap();
        assert!(matches!(next(), Some(Mail::NewSignIn { .. })));
        accounts.handle([2; 32], AccountOp::SetLogin { login: setup("a@b.de", 3) }).unwrap();
        assert_eq!(next(), Some(Mail::PasswordChanged { to: "a@b.de".into() }));
        accounts.handle([2; 32], AccountOp::SetLogin { login: setup("neu@b.de", 4) }).unwrap();
        assert!(matches!(next(), Some(Mail::Verify { to, .. }) if to == "neu@b.de"));
        assert_eq!(next(), Some(Mail::AddressChanged { to: "a@b.de".into(), new: "neu@b.de".into() }));
        assert!(matches!(
            accounts.handle([2; 32], AccountOp::LoginStatus),
            Ok(AccountReply::LoginStatus(Some(LoginInfo { verified: false, .. })))
        ));

        // Recovery is reported; resending is throttled.
        accounts.login_web("neu@b.de", &[104; 32], true).unwrap();
        assert_eq!(next(), Some(Mail::RecoveryUsed { to: "neu@b.de".into() }));
        let id = accounts.by_email["neu@b.de"];
        accounts.resend_verification(id).unwrap();
        assert!(matches!(next(), Some(Mail::Verify { .. })));
        assert_eq!(accounts.resend_verification(id), Err(AccountError::RateLimited));
        assert_eq!(next(), None);
    }

    #[test]
    fn unconfirmed_addresses_go_to_the_next_after_two_days() {
        let mut accounts = Accounts::open(temp(), [0; 32]).unwrap();
        // A stranger signs up with someone else's address first.
        let squatter = accounts.register_web(setup("opfer@b.de", 1)).unwrap();
        assert_eq!(accounts.register_web(setup("opfer@b.de", 2)).err(), Some(AccountError::EmailTaken));
        // Two days later, unconfirmed, it is free again.
        accounts.stored.accounts.get_mut(&squatter).unwrap().login.as_mut().unwrap().since -= VERIFY_SECS + 1;
        let owner = accounts.register_web(setup("opfer@b.de", 2)).unwrap();
        assert_ne!(owner, squatter);
        assert!(accounts.stored.accounts[&squatter].login.is_none());
        assert_eq!(accounts.login_web("opfer@b.de", &[1; 32], false).err(), Some(AccountError::WrongPassword));
        assert_eq!(accounts.login_web("opfer@b.de", &[2; 32], false).unwrap().0, owner);

        // A confirmed address stays taken for good.
        let login = accounts.stored.accounts.get_mut(&owner).unwrap().login.as_mut().unwrap();
        login.verified = true;
        login.since = 0;
        assert_eq!(accounts.register_web(setup("opfer@b.de", 3)).err(), Some(AccountError::EmailTaken));
    }

    #[test]
    fn wrong_passwords_lock_the_account() {
        let mut accounts = Accounts::open(temp(), [0; 32]).unwrap();
        accounts.handle([1; 32], AccountOp::Register { login: setup("a@b.de", 1) }).unwrap();
        for _ in 0..MAX_FAILURES {
            let wrong = AccountOp::Login { email: "a@b.de".into(), auth: [0; 32] };
            assert_eq!(accounts.handle([2; 32], wrong), Err(AccountError::WrongPassword));
        }
        let right = AccountOp::Login { email: "a@b.de".into(), auth: [1; 32] };
        assert_eq!(accounts.handle([2; 32], right), Err(AccountError::Locked));
        // The owner still gets in with the recovery code.
        let recover = AccountOp::Recover { email: "a@b.de".into(), recovery_auth: [101; 32] };
        assert!(matches!(accounts.handle([3; 32], recover), Ok(AccountReply::LoggedIn { .. })));
    }

    #[test]
    fn guessing_ends_the_pairing() {
        let mut accounts = Accounts::open(temp(), [0; 32]).unwrap();
        accounts.handle([1; 32], AccountOp::Create).unwrap();
        accounts.handle([1; 32], offer("ABCD")).unwrap();
        for _ in 0..MAX_TRIES {
            let wrong = AccountOp::JoinPairing { code_id: "ABCD".into(), proof: [0; 32] };
            assert_eq!(accounts.handle([2; 32], wrong), Err(AccountError::WrongCode));
        }
        let right = AccountOp::JoinPairing { code_id: "ABCD".into(), proof: [7; 32] };
        assert_eq!(accounts.handle([2; 32], right), Err(AccountError::UnknownCode));
        // Another account cannot take over a code that is in use.
        accounts.handle([3; 32], AccountOp::Create).unwrap();
        accounts.handle([1; 32], offer("WXYZ")).unwrap();
        assert_eq!(accounts.handle([3; 32], offer("WXYZ")), Err(AccountError::Conflict));
    }
}
