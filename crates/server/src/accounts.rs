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
use serde::{Deserialize, Serialize};

/// Wrong proofs a pairing survives; then it is gone and needs a new code.
const MAX_TRIES: u32 = 3;
/// Wrong passwords (or recovery codes) per account before it locks for [`LOCKOUT`].
const MAX_FAILURES: u32 = 10;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// Longest encrypted device name.
const MAX_LABEL: usize = 512;

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
        })
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
        Ok(Self { path, stored, by_key, pairings: HashMap::new(), by_email, failures: HashMap::new(), pepper })
    }

    /// Carries out a request whose signature the caller has checked.
    pub fn handle(&mut self, public_key: [u8; 32], op: AccountOp) -> Result<AccountReply, AccountError> {
        let key = hex::encode(public_key);
        let now = Instant::now();
        self.pairings.retain(|_, p| p.expires > now);
        let member = self.by_key.get(&key).copied();

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
                let login = StoredLogin::from_setup(login, false)?;
                if self.by_email.contains_key(&login.email) {
                    return Err(AccountError::EmailTaken);
                }
                let id = self.stored.next.max(1);
                let before = self.snapshot();
                self.stored.next = id + 1;
                let wrapped = hex::decode(&login.wrapped).unwrap_or_default();
                self.by_email.insert(login.email.clone(), id);
                self.stored.accounts.insert(id, Account { keys: vec![key.clone()], login: Some(login), ..Default::default() });
                self.by_key.insert(key, id);
                self.commit(before)?;
                Ok(AccountReply::LoggedIn { info: self.info(id), wrapped })
            }
            AccountOp::SetLogin { login } => {
                let id = member.ok_or(AccountError::NotLinked)?;
                let old = self.stored.accounts[&id].login.clone();
                let same_address = old.as_ref().is_some_and(|o| normalize_email(&login.email).as_deref() == Some(o.email.as_str()));
                let login = StoredLogin::from_setup(login, same_address && old.as_ref().is_some_and(|o| o.verified))?;
                if self.by_email.get(&login.email).is_some_and(|owner| *owner != id) {
                    return Err(AccountError::EmailTaken);
                }
                let before = self.snapshot();
                if let Some(old) = &old {
                    self.by_email.remove(&old.email);
                }
                self.by_email.insert(login.email.clone(), id);
                self.stored.accounts.get_mut(&id).expect("member of it").login = Some(login);
                self.commit(before)?;
                self.failures.remove(&id);
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
        let email = normalize_email(email).ok_or(AccountError::WrongPassword)?;
        let id = *self.by_email.get(&email).ok_or(AccountError::WrongPassword)?;
        if member.is_some_and(|m| m != id) {
            return Err(AccountError::AlreadyLinked);
        }
        let now = Instant::now();
        let failures = self.failures.entry(id).or_insert((0, now));
        if now.duration_since(failures.1) > LOCKOUT {
            *failures = (0, now);
        }
        if failures.0 >= MAX_FAILURES {
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
            failures.0 += 1;
            return Err(AccountError::WrongPassword);
        }
        self.failures.remove(&id);
        if member.is_none() {
            if self.stored.accounts[&id].keys.len() >= MAX_DEVICES {
                return Err(AccountError::TooManyDevices);
            }
            let before = self.snapshot();
            self.stored.accounts.get_mut(&id).expect("exists").keys.push(key.clone());
            self.by_key.insert(key, id);
            self.commit(before)?;
        }
        Ok(AccountReply::LoggedIn { info: self.info(id), wrapped: hex::decode(wrapped).unwrap_or_default() })
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
    fn wrong_passwords_lock_the_account() {
        let mut accounts = Accounts::open(temp(), [0; 32]).unwrap();
        accounts.handle([1; 32], AccountOp::Register { login: setup("a@b.de", 1) }).unwrap();
        for _ in 0..MAX_FAILURES {
            let wrong = AccountOp::Login { email: "a@b.de".into(), auth: [0; 32] };
            assert_eq!(accounts.handle([2; 32], wrong), Err(AccountError::WrongPassword));
        }
        let right = AccountOp::Login { email: "a@b.de".into(), auth: [1; 32] };
        assert_eq!(accounts.handle([2; 32], right), Err(AccountError::Locked));
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
