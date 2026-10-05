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
    valid_code_id, AccountError, AccountInfo, AccountOp, AccountReply, MAX_BOOK, MAX_DEVICES, PAIRING_SECS,
};
use serde::{Deserialize, Serialize};

/// Wrong proofs a pairing survives; then it is gone and needs a new code.
const MAX_TRIES: u32 = 3;

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
}

impl Accounts {
    pub fn open(path: PathBuf) -> Result<Self> {
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
        Ok(Self { path, stored, by_key, pairings: HashMap::new() })
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
            AccountOp::Leave => {
                let id = member.ok_or(AccountError::NotLinked)?;
                let before = self.snapshot();
                let account = self.stored.accounts.get_mut(&id).expect("member of it");
                account.keys.retain(|k| *k != key);
                if account.keys.is_empty() {
                    self.stored.accounts.remove(&id);
                    self.pairings.retain(|_, p| p.account != id);
                }
                self.by_key.remove(&key);
                self.commit(before)?;
                Ok(AccountReply::Left)
            }
        }
    }

    fn info(&self, id: u64) -> AccountInfo {
        AccountInfo { devices: self.stored.accounts.get(&id).map_or(0, |a| a.keys.len() as u32) }
    }

    fn snapshot(&self) -> (Stored, HashMap<String, u64>) {
        (Stored { next: self.stored.next, accounts: self.stored.accounts.clone() }, self.by_key.clone())
    }

    /// Saves; on failure memory goes back to `before`, so it never disagrees with disk.
    fn commit(&mut self, before: (Stored, HashMap<String, u64>)) -> Result<(), AccountError> {
        if let Err(e) = self.save() {
            tracing::warn!("Konten nicht gespeichert: {e:#}");
            (self.stored, self.by_key) = before;
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
        let mut accounts = Accounts::open(path.clone()).unwrap();
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
        let mut reopened = Accounts::open(path.clone()).unwrap();
        assert_eq!(reopened.handle(b, AccountOp::Status), Ok(AccountReply::Status(Some(AccountInfo { devices: 2 }))));

        // The last device to leave deletes the account.
        assert_eq!(reopened.handle(a, AccountOp::Leave), Ok(AccountReply::Left));
        assert_eq!(reopened.handle(b, AccountOp::Status), Ok(AccountReply::Status(Some(AccountInfo { devices: 1 }))));
        assert_eq!(reopened.handle(b, AccountOp::Leave), Ok(AccountReply::Left));
        assert!(reopened.stored.accounts.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn guessing_ends_the_pairing() {
        let mut accounts = Accounts::open(temp()).unwrap();
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
