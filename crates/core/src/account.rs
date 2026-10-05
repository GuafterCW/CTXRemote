//! Accounts on the client: create, pair devices with a one-time code, and
//! keep the address book (`Config::peers`) the same on all of them.
//!
//! The address book travels encrypted with the account key, which only the
//! member devices hold; a pairing hands it over sealed with a key derived
//! from the code's secret part (Argon2id). See `docs/ACCOUNTS.md`.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use ctxremote_proto::account::{
    self as wire, AccountError, AccountInfo, AccountOp, AccountReply, Kdf, LoginInfo, LoginSetup, CODE_ALPHABET,
    CODE_ID_LEN,
};
use hkdf::Hkdf;
use sha2::Sha256;
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::{ClientMsg, ServerMsg};
use ctxremote_proto::DeviceId;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::config::{Config, Peer};
use crate::net;

const STEP_TIMEOUT: Duration = Duration::from_secs(15);
const BOOK_AAD: &[u8] = b"ctxremote/book/v1";
const SEAL_AAD: &[u8] = b"ctxremote/pairing/v1";
const LOGIN_AAD: &[u8] = b"ctxremote/login/v1";
const RECOVERY_AAD: &[u8] = b"ctxremote/recovery/v1";
const LABEL_AAD: &[u8] = b"ctxremote/label/v1";
/// Shortest password accepted for a login.
pub const MIN_PASSWORD: usize = 10;
/// Characters of a recovery code (125 bits).
const RECOVERY_LEN: usize = 25;
/// Tombstones of removed devices are kept this long, then forgotten.
const TOMBSTONE_MS: u64 = 90 * 24 * 3600 * 1000;

/// What a device keeps to take part in an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountLink {
    /// The address-book key, hex-encoded; only member devices have it.
    pub key: String,
    /// Members at the last contact, for display.
    #[serde(default)]
    pub devices: u32,
}

impl AccountLink {
    fn key(&self) -> Result<[u8; 32]> {
        hex::decode(&self.key).ok().and_then(|k| k.try_into().ok()).context("Kontoschlüssel ist beschädigt")
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Sends one signed request and returns the server's answer.
async fn request(server: &str, key: &SigningKey, op: AccountOp) -> Result<AccountReply> {
    let (mut t, nonce) = net::dial(server).await?;
    let auth = wire::sign(key, &nonce, &op);
    framing::send(&mut t, &ClientMsg::Account { auth, op }).await?;
    match timeout(STEP_TIMEOUT, framing::recv::<ServerMsg>(&mut t)).await? {
        Ok(ServerMsg::Account(result)) => Ok(result?),
        Ok(ServerMsg::Error(e)) => Err(e.into()),
        Ok(other) => bail!("unerwartete Serverantwort: {other:?}"),
        // Servers from before accounts hang up on the unknown request.
        Err(_) => bail!("Der Server kennt noch keine Konten (Server-Update nötig)"),
    }
}

pub async fn status(server: &str, key: &SigningKey) -> Result<Option<AccountInfo>> {
    match request(server, key, AccountOp::Status).await? {
        AccountReply::Status(info) => Ok(info),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Creates an account with this device as its only member.
pub async fn create(server: &str, key: &SigningKey) -> Result<AccountLink> {
    match request(server, key, AccountOp::Create).await? {
        AccountReply::Created(info) => {
            Ok(AccountLink { key: hex::encode(rand::random::<[u8; 32]>()), devices: info.devices })
        }
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Opens a pairing and returns the code to type on the new device, as
/// `ABCD-EFGH-JKMN`. It is valid for [`wire::PAIRING_SECS`] and once.
pub async fn offer_pairing(server: &str, key: &SigningKey, link: &AccountLink) -> Result<String> {
    let account_key = link.key()?;
    for _ in 0..5 {
        let code: String = (0..CODE_ID_LEN + wire::CODE_SECRET_LEN)
            .map(|_| CODE_ALPHABET[rand::random::<usize>() % CODE_ALPHABET.len()] as char)
            .collect();
        let (code_id, secret) = code.split_at(CODE_ID_LEN);
        let salt: [u8; 16] = rand::random();
        let (seal_key, verifier) = derive(secret, &salt)?;
        let sealed = seal(&seal_key, SEAL_AAD, &account_key)?;
        let op = AccountOp::OfferPairing { code_id: code_id.into(), salt, verifier, sealed };
        match request(server, key, op).await {
            Ok(_) => return Ok(format!("{}-{}-{}", &code[..4], &code[4..8], &code[8..])),
            // Someone else's pairing uses this code name right now: pick another.
            Err(e) if e.downcast_ref::<AccountError>() == Some(&AccountError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Kein freier Code gefunden, bitte nochmal versuchen")
}

/// Joins the account whose member showed `code`.
pub async fn join(server: &str, key: &SigningKey, code: &str) -> Result<AccountLink> {
    let code = wire::normalize_code(code).context("Der Code hat 12 Zeichen, z. B. ABCD-EFGH-JKMN")?;
    let (code_id, secret) = code.split_at(CODE_ID_LEN);
    let salt = match request(server, key, AccountOp::PairingSalt { code_id: code_id.into() }).await? {
        AccountReply::Salt(salt) => salt,
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    let (seal_key, proof) = derive(secret, &salt)?;
    match request(server, key, AccountOp::JoinPairing { code_id: code_id.into(), proof }).await? {
        AccountReply::Joined { info, sealed } => {
            let account_key: [u8; 32] = open(&seal_key, SEAL_AAD, &sealed)?
                .try_into()
                .map_err(|_| anyhow!("Kontoschlüssel ist beschädigt"))?;
            Ok(AccountLink { key: hex::encode(account_key), devices: info.devices })
        }
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Takes this device out of its account. Its local address book stays.
pub async fn leave(server: &str, key: &SigningKey) -> Result<()> {
    match request(server, key, AccountOp::Leave).await {
        Ok(_) => Ok(()),
        // Already out (e.g. removed elsewhere): the goal is reached.
        Err(e) if e.downcast_ref::<AccountError>() == Some(&AccountError::NotLinked) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Merges `local` with the account's address book, stores the result there
/// and returns it. Retries when another device wrote in between.
pub async fn sync(server: &str, key: &SigningKey, link: &AccountLink, local: &Book) -> Result<(Book, u32)> {
    let account_key = link.key()?;
    for _ in 0..4 {
        let (revision, blob) = match request(server, key, AccountOp::GetBook).await? {
            AccountReply::Book { revision, blob } => (revision, blob),
            other => bail!("unerwartete Serverantwort: {other:?}"),
        };
        let remote = if blob.is_empty() {
            Book::default()
        } else {
            serde_json::from_slice(&open(&account_key, BOOK_AAD, &blob)?).context("Adressbuch ist beschädigt")?
        };
        let merged = Book::merge(local, &remote);
        if merged == remote && revision > 0 {
            return Ok((merged, link.devices));
        }
        let blob = seal(&account_key, BOOK_AAD, &serde_json::to_vec(&merged)?)?;
        match request(server, key, AccountOp::PutBook { base: revision, blob }).await {
            Ok(_) => return Ok((merged, link.devices)),
            Err(e) if e.downcast_ref::<AccountError>() == Some(&AccountError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Das Adressbuch ändert sich gerade ständig, bitte später nochmal versuchen")
}

/// Argon2id over the code's secret part: a key to seal the account key and,
/// independently, the proof the server checks.
fn derive(secret: &str, salt: &[u8; 16]) -> Result<([u8; 32], [u8; 32])> {
    let params = Params::new(19 * 1024, 2, 1, Some(64)).map_err(|e| anyhow!("{e}"))?;
    let mut out = [0u8; 64];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(secret.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow!("{e}"))?;
    let (seal_key, proof) = out.split_at(32);
    Ok((seal_key.try_into().expect("32 bytes"), proof.try_into().expect("32 bytes")))
}

fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; 12] = rand::random();
    let sealed = ChaCha20Poly1305::new(key.into())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| anyhow!("Verschlüsselung fehlgeschlagen"))?;
    Ok([nonce.as_slice(), &sealed].concat())
}

fn open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 12 {
        bail!("Daten sind beschädigt");
    }
    let (nonce, body) = sealed.split_at(12);
    ChaCha20Poly1305::new(key.into())
        .decrypt(Nonce::from_slice(nonce), Payload { msg: body, aad })
        .map_err(|_| anyhow!("Entschlüsseln fehlgeschlagen: falscher Code oder falscher Kontoschlüssel"))
}

/// The address book as it is shared: devices plus removals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    /// Device ID → entry.
    pub entries: BTreeMap<u32, Entry>,
    /// Device ID → Unix milliseconds of its removal.
    pub removed: BTreeMap<u32, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub alias: Option<String>,
    /// When the alias was last changed (Unix ms); the newer one wins.
    pub alias_at: u64,
    pub name: String,
    /// Unix seconds; the newer connection brings its name along.
    pub last_seen: u64,
}

impl Book {
    pub fn from_config(config: &Config) -> Self {
        Self {
            entries: config
                .peers
                .iter()
                .map(|p| {
                    let entry = Entry { alias: p.alias.clone(), alias_at: p.alias_at, name: p.name.clone(), last_seen: p.last_seen };
                    (p.id.get(), entry)
                })
                .collect(),
            removed: config.removed.iter().map(|(id, at)| (id.get(), *at)).collect(),
        }
    }

    /// Replaces the config's list with this book, most recent first.
    pub fn apply_to(&self, config: &mut Config) {
        let mut peers: Vec<Peer> = self
            .entries
            .iter()
            .filter_map(|(id, e)| {
                Some(Peer { id: DeviceId::new(*id)?, alias: e.alias.clone(), name: e.name.clone(), last_seen: e.last_seen, alias_at: e.alias_at })
            })
            .collect();
        peers.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        config.peers = peers;
        config.removed = self.removed.iter().filter_map(|(id, at)| Some((DeviceId::new(*id)?, *at))).collect();
        config.prune();
    }

    /// Field by field: the newer alias, the newer connection, and a removal
    /// only for entries that did not change after it.
    pub fn merge(a: &Self, b: &Self) -> Self {
        let cutoff = now_ms().saturating_sub(TOMBSTONE_MS);
        let mut removed = a.removed.clone();
        for (id, at) in &b.removed {
            let slot = removed.entry(*id).or_default();
            *slot = (*slot).max(*at);
        }
        removed.retain(|_, at| *at >= cutoff);

        let mut entries = BTreeMap::new();
        for id in a.entries.keys().chain(b.entries.keys()) {
            let entry = match (a.entries.get(id), b.entries.get(id)) {
                (Some(x), Some(y)) => {
                    let alias_from = if y.alias_at > x.alias_at { y } else { x };
                    let seen_from = if y.last_seen > x.last_seen { y } else { x };
                    let name = if seen_from.name.is_empty() { alias_from.name.clone() } else { seen_from.name.clone() };
                    Entry { alias: alias_from.alias.clone(), alias_at: alias_from.alias_at, name, last_seen: seen_from.last_seen }
                }
                (Some(x), None) | (None, Some(x)) => x.clone(),
                (None, None) => continue,
            };
            let changed = entry.alias_at.max(entry.last_seen.saturating_mul(1000));
            if removed.get(id).is_some_and(|at| *at >= changed) {
                continue;
            }
            entries.insert(*id, entry);
        }
        Self { entries, removed }
    }
}

// ---- Logins (e-mail and password), see docs/ACCOUNTS.md ----

/// The password's two independent values: `auth` for the server, `wrap` to
/// seal the account key. Argon2id makes guessing expensive.
fn password_keys(password: &str, salt: &[u8; 16], kdf: &Kdf) -> Result<([u8; 32], [u8; 32])> {
    if !kdf.acceptable() {
        bail!("Der Server verlangt ungültige Einstellungen für die Schlüsselableitung");
    }
    let params = Params::new(kdf.memory_kib, kdf.iterations, kdf.parallelism, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let mut master = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut master)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(split(&master, b"ctxremote/login/v1"))
}

/// The recovery code's values; it has 125 random bits, so no slow hash is needed.
fn recovery_keys(code: &str) -> Result<([u8; 32], [u8; 32])> {
    let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    if code.len() != RECOVERY_LEN || !code.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        bail!("Der Wiederherstellungscode hat 25 Zeichen, z. B. ABCDE-FGHJK-MNPQR-STVWX-YZ012");
    }
    Ok(split(code.as_bytes(), b"ctxremote/recovery/v1"))
}

fn split(ikm: &[u8], salt: &[u8]) -> ([u8; 32], [u8; 32]) {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let (mut auth, mut wrap) = ([0u8; 32], [0u8; 32]);
    hk.expand(b"auth", &mut auth).expect("32 bytes");
    hk.expand(b"wrap", &mut wrap).expect("32 bytes");
    (auth, wrap)
}

/// A fresh recovery code, grouped as `ABCDE-FGHJK-MNPQR-STVWX-YZ012`.
pub fn new_recovery_code() -> String {
    let chars: Vec<char> =
        (0..RECOVERY_LEN).map(|_| CODE_ALPHABET[rand::random::<usize>() % CODE_ALPHABET.len()] as char).collect();
    chars.chunks(5).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

fn check_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD {
        bail!("Das Passwort braucht mindestens {MIN_PASSWORD} Zeichen");
    }
    Ok(())
}

/// The login for `account_key`, with a new recovery code (returned).
fn login_setup(email: &str, password: &str, account_key: &[u8; 32]) -> Result<(LoginSetup, String)> {
    let email = wire::normalize_email(email).ok_or(AccountError::InvalidEmail)?;
    check_password(password)?;
    let salt: [u8; 16] = rand::random();
    let kdf = Kdf::CURRENT;
    let (auth, wrap) = password_keys(password, &salt, &kdf)?;
    let code = new_recovery_code();
    let (recovery_auth, recovery_wrap) = recovery_keys(&code)?;
    let setup = LoginSetup {
        email,
        salt,
        kdf,
        auth,
        wrapped: seal(&wrap, LOGIN_AAD, account_key)?,
        recovery_auth,
        recovery_wrapped: seal(&recovery_wrap, RECOVERY_AAD, account_key)?,
    };
    Ok((setup, code))
}

fn unwrap_key(wrap: &[u8; 32], aad: &[u8], wrapped: &[u8]) -> Result<[u8; 32]> {
    open(wrap, aad, wrapped)?.try_into().map_err(|_| anyhow!("Kontoschlüssel ist beschädigt"))
}

/// Creates an account with a login and this device. Returns the link and the
/// recovery code, which must be shown to the user once.
pub async fn register(server: &str, key: &SigningKey, email: &str, password: &str) -> Result<(AccountLink, String)> {
    let account_key: [u8; 32] = rand::random();
    let (login, code) = login_setup(email, password, &account_key)?;
    match request(server, key, AccountOp::Register { login }).await? {
        AccountReply::LoggedIn { info, .. } => Ok((AccountLink { key: hex::encode(account_key), devices: info.devices }, code)),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Adds this device to the account of `email`.
pub async fn login(server: &str, key: &SigningKey, email: &str, password: &str) -> Result<AccountLink> {
    let (salt, kdf) = match request(server, key, AccountOp::Prelogin { email: email.into() }).await? {
        AccountReply::Prelogin { salt, kdf } => (salt, kdf),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    let (auth, wrap) = password_keys(password, &salt, &kdf)?;
    match request(server, key, AccountOp::Login { email: email.into(), auth }).await? {
        AccountReply::LoggedIn { info, wrapped } => {
            Ok(AccountLink { key: hex::encode(unwrap_key(&wrap, LOGIN_AAD, &wrapped)?), devices: info.devices })
        }
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Adds this device with the recovery code and sets `new_password`. Returns
/// the link and a new recovery code (the old one stops working).
pub async fn recover(
    server: &str,
    key: &SigningKey,
    email: &str,
    code: &str,
    new_password: &str,
) -> Result<(AccountLink, String)> {
    check_password(new_password)?;
    let (recovery_auth, recovery_wrap) = recovery_keys(code)?;
    let (info, account_key) = match request(server, key, AccountOp::Recover { email: email.into(), recovery_auth }).await? {
        AccountReply::LoggedIn { info, wrapped } => (info, unwrap_key(&recovery_wrap, RECOVERY_AAD, &wrapped)?),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    let link = AccountLink { key: hex::encode(account_key), devices: info.devices };
    let code = set_login(server, key, &link, email, new_password).await?;
    Ok((link, code))
}

/// Adds a login to this device's account or changes it (password or
/// address). Returns the new recovery code.
pub async fn set_login(server: &str, key: &SigningKey, link: &AccountLink, email: &str, password: &str) -> Result<String> {
    let (login, code) = login_setup(email, password, &link.key()?)?;
    match request(server, key, AccountOp::SetLogin { login }).await? {
        AccountReply::Done => Ok(code),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

pub async fn login_status(server: &str, key: &SigningKey) -> Result<Option<LoginInfo>> {
    match request(server, key, AccountOp::LoginStatus).await? {
        AccountReply::LoginStatus(info) => Ok(info),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// A device of the account as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDevice {
    /// Hex; identifies it for [`remove_device`].
    pub public_key: String,
    /// Its ID, e.g. `482 913 077`, if registered.
    pub id: Option<String>,
    pub online: bool,
    /// Its own name, decrypted; empty if it set none.
    pub name: String,
    /// This device.
    pub this: bool,
}

pub async fn devices(server: &str, key: &SigningKey, link: &AccountLink) -> Result<Vec<AccountDevice>> {
    let account_key = link.key()?;
    let own = key.verifying_key().to_bytes();
    match request(server, key, AccountOp::Devices).await? {
        AccountReply::Devices(members) => Ok(members
            .into_iter()
            .map(|m| AccountDevice {
                public_key: hex::encode(m.public_key),
                id: m.device.map(|d| d.to_string()),
                online: m.online,
                name: open(&account_key, LABEL_AAD, &m.label)
                    .ok()
                    .and_then(|n| String::from_utf8(n).ok())
                    .unwrap_or_default(),
                this: m.public_key == own,
            })
            .collect()),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Takes another device out of the account (it keeps its local list).
pub async fn remove_device(server: &str, key: &SigningKey, public_key: &str) -> Result<()> {
    let public_key: [u8; 32] = hex::decode(public_key).ok().and_then(|k| k.try_into().ok()).context("ungültiges Gerät")?;
    match request(server, key, AccountOp::RemoveDevice { public_key }).await? {
        AccountReply::Done => Ok(()),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// The name a device gives itself in the account: its computer name.
pub fn default_label() -> String {
    whoami::devicename()
}

/// This device's name for the other devices of the account (encrypted).
pub async fn set_label(server: &str, key: &SigningKey, link: &AccountLink, name: &str) -> Result<()> {
    let name: String = name.trim().chars().take(80).collect();
    let label = if name.is_empty() { Vec::new() } else { seal(&link.key()?, LABEL_AAD, name.as_bytes())? };
    match request(server, key, AccountOp::SetLabel { label }).await? {
        AccountReply::Done => Ok(()),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(alias: Option<&str>, alias_at: u64, name: &str, last_seen: u64) -> Entry {
        Entry { alias: alias.map(Into::into), alias_at, name: name.into(), last_seen }
    }

    #[test]
    fn merge_takes_the_newer_parts() {
        let now = now_ms();
        let id = 482_913_077;
        let mut a = Book::default();
        let mut b = Book::default();
        // a renamed later, b connected later: both changes survive.
        a.entries.insert(id, entry(Some("Büro"), now, "OLD-NAME", 100));
        b.entries.insert(id, entry(Some("Alt"), now - 10, "NEW-NAME", 200));
        // Only on one side.
        b.entries.insert(731_004_552, entry(Some("Mama"), now, "", 0));
        let merged = Book::merge(&a, &b);
        assert_eq!(merged.entries[&id], entry(Some("Büro"), now, "NEW-NAME", 200));
        assert!(merged.entries.contains_key(&731_004_552));
        assert_eq!(merged, Book::merge(&b, &a), "order does not matter");

        // A removal wins over older entries, not over later changes.
        let mut gone = merged.clone();
        gone.entries.remove(&id);
        gone.removed.insert(id, now + 5);
        assert!(!Book::merge(&merged, &gone).entries.contains_key(&id));
        let mut renamed_after = merged.clone();
        renamed_after.entries.get_mut(&id).unwrap().alias_at = now + 10;
        assert!(Book::merge(&renamed_after, &gone).entries.contains_key(&id));

        // Ancient tombstones are dropped.
        let mut old = Book::default();
        old.removed.insert(1, 5);
        assert!(Book::merge(&old, &Book::default()).removed.is_empty());
    }

    #[test]
    fn config_round_trip() {
        let mut config = Config::default();
        let id: DeviceId = "482913077".parse().unwrap();
        config.remember(id, "BUERO-PC");
        config.set_alias(id, Some("Büro")).unwrap();
        config.forget("731004552".parse().unwrap());
        let book = Book::from_config(&config);
        let mut other = Config::default();
        book.apply_to(&mut other);
        assert_eq!(other.peers.len(), 1);
        assert_eq!(other.peers[0].alias.as_deref(), Some("Büro"));
        assert_eq!(other.removed.len(), 1);
    }

    #[test]
    fn login_keys() {
        let kdf = Kdf { memory_kib: 19 * 1024, iterations: 2, parallelism: 1 };
        let (auth, wrap) = password_keys("richtig-langes-pw", &[1; 16], &kdf).unwrap();
        assert_ne!(auth, wrap);
        assert_eq!(password_keys("richtig-langes-pw", &[1; 16], &kdf).unwrap(), (auth, wrap));
        assert_ne!(password_keys("richtig-langes-pX", &[1; 16], &kdf).unwrap().0, auth);
        assert!(password_keys("x", &[1; 16], &Kdf { memory_kib: 8, ..kdf }).is_err());

        let code = new_recovery_code();
        assert_eq!(code.len(), 29, "{code}");
        assert_eq!(recovery_keys(&code).unwrap(), recovery_keys(&code.to_lowercase().replace('-', " ")).unwrap());
        assert!(recovery_keys("ABC").is_err());
    }

    /// The browser (`web/scripts/vectors.mjs`) checks itself against the same
    /// values, so both always derive identical keys.
    #[test]
    fn vectors_stay_stable() {
        let kdf = Kdf { memory_kib: 19 * 1024, iterations: 2, parallelism: 1 };
        let (auth, wrap) = password_keys("correct horse battery", &[1; 16], &kdf).unwrap();
        assert_eq!(hex::encode(auth), "bfeaa4346470058b73858a7f1a22b7c14bc7333469cbc3669b3190ffe86cbe61");
        assert_eq!(hex::encode(wrap), "43c99233e76be19472996ce0fb75cf6c3f3482c3dd7031a2bd04cfc4dc83a933");
        let (auth, wrap) = recovery_keys("ABCDE-FGHJK-MNPQR-STVWX-YZ012").unwrap();
        assert_eq!(hex::encode(auth), "94ef0956acd1674365fdc35d6c457da4d2e3a93b672ad04c3b6f3200c4ddaa62");
        assert_eq!(hex::encode(wrap), "04f1a5a986fa44b667cf0b7a9b7f46e308b54f5298b52b2c03b26651dd359428");
        let (seal_key, proof) = derive("EFGHJKMN", &[2; 16]).unwrap();
        assert_eq!(hex::encode(seal_key), "68c882c12cf480262a38328764a02873a37745a1787bf8b439eae5c7566943d4");
        assert_eq!(hex::encode(proof), "38149ca52fcf37b53161554d801b005e73525581ceebef0495687178b77fa16e");
        let sealed = hex::decode("0407bad1d051ff1bcfd6380aded890e7ea4e8e4e9e6e067c700dd820394995354249f07f449eb17a66a556387009ea44f1b1c014e3785b").unwrap();
        assert_eq!(open(&[5; 32], BOOK_AAD, &sealed).unwrap(), br#"{"entries":{},"removed":{}}"#);
    }

    #[test]
    fn sealing() {
        let (key, proof) = derive("EFGHJKMN", &[1; 16]).unwrap();
        let (again, _) = derive("EFGHJKMN", &[1; 16]).unwrap();
        assert_eq!(key, again);
        assert_ne!(key, proof);
        let (other, _) = derive("EFGHJKMP", &[1; 16]).unwrap();
        let sealed = seal(&key, SEAL_AAD, b"geheim").unwrap();
        assert_eq!(open(&key, SEAL_AAD, &sealed).unwrap(), b"geheim");
        assert!(open(&other, SEAL_AAD, &sealed).is_err());
        assert!(open(&key, BOOK_AAD, &sealed).is_err());
    }
}
