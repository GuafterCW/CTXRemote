//! Teams on the client: several accounts sharing an address book, with
//! roles the server enforces (see `docs/ACCOUNTS.md`, "Teams").
//!
//! The team key never reaches the server. Members get it sealed for their
//! account's exchange key (X25519, derived from the account key), which
//! carries a tag only holders of the team key can make, so the server cannot
//! slip in a key of its own. Removing someone moves the team to a new epoch
//! with a new key.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use ctxremote_proto::account::{
    self as wire, AccountError, AccountOp, AccountReply, MemberKey, Rotation, TeamMember, TeamRole, TeamState,
    CODE_ALPHABET, CODE_ID_LEN,
};
use ctxremote_proto::DeviceId;
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::account::{derive, now_ms, open, request, seal, AccountLink, Book};

const BOOK_AAD: &[u8] = b"ctxremote/team-book/v1";
const ACCESS_AAD: &[u8] = b"ctxremote/team-access-list/v1";
const INVITE_AAD: &[u8] = b"ctxremote/team-invite/v1";
const BOX_AAD: &[u8] = b"ctxremote/team-box/v1";

/// What a device keeps about its account's team: the current key, so
/// access passwords work without asking the server first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamLink {
    /// Hex.
    pub id: String,
    pub epoch: u32,
    /// The team key, hex-encoded.
    pub key: String,
    /// This account's member ID.
    pub me: u64,
    pub role: TeamRole,
}

impl TeamLink {
    pub(crate) fn key(&self) -> Result<[u8; 32]> {
        hex::decode(&self.key).ok().and_then(|k| k.try_into().ok()).context("Teamschlüssel ist beschädigt")
    }
}

/// The shared address book: the team's name and its devices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamBook {
    pub name: String,
    #[serde(default)]
    pub book: Book,
}

/// One member's devices that let the team in without a password.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessList {
    /// Device ID → its name and when it was opened (Unix ms).
    pub hosts: BTreeMap<u32, (String, u64)>,
}

/// The team as the app shows it, decrypted.
#[derive(Debug, Clone)]
pub struct TeamView {
    pub link: TeamLink,
    pub revision: u64,
    pub book: TeamBook,
    pub members: Vec<TeamMember>,
    /// Members whose key the team key does not vouch for: just joined and
    /// not yet confirmed, or slipped in. They are left out of new epochs.
    pub unverified: Vec<u64>,
    /// Someone left; the next admin's sync starts a new epoch.
    pub stale: bool,
    /// Device ID → (member who opened it, its name).
    pub access: BTreeMap<u32, (u64, String)>,
}

fn context(id: &[u8; 16], epoch: u32) -> Vec<u8> {
    [id.as_slice(), &epoch.to_be_bytes()].concat()
}

fn aad(prefix: &[u8], id: &[u8; 16], epoch: u32) -> Vec<u8> {
    [prefix, &context(id, epoch)].concat()
}

fn access_aad(id: &[u8; 16], epoch: u32, member: u64) -> Vec<u8> {
    [aad(ACCESS_AAD, id, epoch).as_slice(), &member.to_be_bytes()].concat()
}

/// The account's X25519 secret, the same on all its devices and in the browser.
pub fn exchange_secret(account_key: &[u8; 32]) -> StaticSecret {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"ctxremote/team-kx/v1"), account_key).expand(b"x25519", &mut out).expect("32 bytes");
    StaticSecret::from(out)
}

pub fn exchange_public(account_key: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&exchange_secret(account_key)).to_bytes()
}

/// Seals `plain` for the holder of `public`'s secret: a fresh key pair,
/// ECDH, HKDF, ChaCha20-Poly1305. Bound to the team and epoch.
fn seal_to(public: &[u8; 32], ctx: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let ephemeral = StaticSecret::from(rand::random::<[u8; 32]>());
    let ephemeral_public = PublicKey::from(&ephemeral).to_bytes();
    let shared = ephemeral.diffie_hellman(&PublicKey::from(*public));
    if !shared.was_contributory() {
        bail!("Ungültiger Schlüssel eines Mitglieds");
    }
    let key = box_key(shared.as_bytes(), &ephemeral_public, public);
    Ok([ephemeral_public.as_slice(), &seal(&key, &[BOX_AAD, ctx].concat(), plain)?].concat())
}

fn open_with(secret: &StaticSecret, ctx: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 32 {
        bail!("Teamschlüssel ist beschädigt");
    }
    let (ephemeral_public, body) = sealed.split_at(32);
    let ephemeral_public: [u8; 32] = ephemeral_public.try_into().expect("32 bytes");
    let shared = secret.diffie_hellman(&PublicKey::from(ephemeral_public));
    let key = box_key(shared.as_bytes(), &ephemeral_public, &PublicKey::from(secret).to_bytes());
    open(&key, &[BOX_AAD, ctx].concat(), body)
}

fn box_key(shared: &[u8; 32], ephemeral_public: &[u8; 32], recipient: &[u8; 32]) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(BOX_AAD), shared)
        .expand(&[ephemeral_public.as_slice(), recipient].concat(), &mut key)
        .expect("32 bytes");
    key
}

/// Vouches for `member`'s exchange key; only holders of the team key can make it.
pub fn tag(team_key: &[u8; 32], member: u64, public: &[u8; 32]) -> [u8; 32] {
    let mut tag_key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"ctxremote/team-tag/v1"), team_key).expand(b"tag", &mut tag_key).expect("32 bytes");
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&tag_key).expect("any key length");
    mac.update(&member.to_be_bytes());
    mac.update(public);
    mac.finalize().into_bytes().into()
}

fn tag_ok(team_key: &[u8; 32], member: &TeamMember) -> bool {
    member.key.is_some_and(|(public, given)| {
        let expected = tag(team_key, member.id, &public);
        expected.iter().zip(given).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
    })
}

/// `member`'s key entry for `team_key`, sealed for its exchange key.
fn member_key(team_key: &[u8; 32], ctx: &[u8], member: u64, public: [u8; 32]) -> Result<MemberKey> {
    Ok(MemberKey { public_key: public, tag: tag(team_key, member, &public), sealed: seal_to(&public, ctx, team_key)? })
}

/// The password a team member uses for `host` instead of the host's own
/// (see "Zugriff ohne Passwort im Team"); changes with every epoch.
pub fn access_password(team_key: &[u8; 32], host: DeviceId) -> String {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"ctxremote/team-access/v1"), team_key)
        .expand(&host.get().to_be_bytes(), &mut out)
        .expect("32 bytes");
    hex::encode(out)
}

fn team_id(state: &TeamState) -> [u8; 16] {
    state.id
}

fn unexpected(other: AccountReply) -> anyhow::Error {
    anyhow!("unerwartete Serverantwort: {other:?}")
}

fn is(e: &anyhow::Error, kind: AccountError) -> bool {
    e.downcast_ref::<AccountError>() == Some(&kind)
}

/// Decrypts `state` with `team_key`.
fn view(state: TeamState, team_key: [u8; 32]) -> Result<TeamView> {
    let id = team_id(&state);
    let book = if state.blob.is_empty() {
        TeamBook::default()
    } else {
        serde_json::from_slice(&open(&team_key, &aad(BOOK_AAD, &id, state.epoch), &state.blob)?)
            .context("Team-Adressbuch ist beschädigt")?
    };
    let mut access = BTreeMap::new();
    for (member, blob) in &state.access {
        // A list that does not open is ignored rather than failing the team.
        let Ok(plain) = open(&team_key, &access_aad(&id, state.epoch, *member), blob) else { continue };
        let Ok(list) = serde_json::from_slice::<AccessList>(&plain) else { continue };
        for (host, (name, _)) in list.hosts {
            access.insert(host, (*member, name));
        }
    }
    let role = state.members.iter().find(|m| m.id == state.me).map_or(TeamRole::Technician, |m| m.role);
    let unverified = state.members.iter().filter(|m| !tag_ok(&team_key, m)).map(|m| m.id).collect();
    Ok(TeamView {
        link: TeamLink { id: hex::encode(id), epoch: state.epoch, key: hex::encode(team_key), me: state.me, role },
        revision: state.revision,
        book,
        members: state.members,
        unverified,
        stale: state.stale,
        access,
    })
}

async fn get(server: &str, key: &SigningKey) -> Result<Option<TeamState>> {
    match request(server, key, AccountOp::GetTeam).await? {
        AccountReply::Team(state) => Ok(state),
        other => Err(unexpected(other)),
    }
}

/// The team key from the server's state: sealed for this account, or,
/// right after joining, the one `cached` still holds for this epoch.
fn team_key(account: &AccountLink, state: &TeamState, cached: Option<&TeamLink>) -> Result<[u8; 32]> {
    let ctx = context(&state.id, state.epoch);
    if let Some(sealed) = &state.sealed {
        let plain = open_with(&exchange_secret(&account.key()?), &ctx, sealed)?;
        return plain.try_into().map_err(|_| anyhow!("Teamschlüssel ist beschädigt"));
    }
    match cached {
        Some(link) if link.id == hex::encode(state.id) && link.epoch == state.epoch => link.key(),
        _ => bail!("Der Beitritt zum Team ist nicht abgeschlossen, bitte neu einladen lassen"),
    }
}

/// Creates a team named `name` with this account as its owner.
pub async fn create(server: &str, key: &SigningKey, account: &AccountLink, name: &str) -> Result<TeamView> {
    // The server picks the team's ID, which the blobs are bound to; so the
    // first book goes in empty and the name follows right after.
    let team_key: [u8; 32] = rand::random();
    let public = exchange_public(&account.key()?);
    // Sealed for a placeholder context; replaced by `ConfirmMember` below.
    let placeholder = MemberKey { public_key: public, tag: [0; 32], sealed: Vec::new() };
    let state = match request(server, key, AccountOp::CreateTeam { key: placeholder, blob: Vec::new() }).await? {
        AccountReply::Team(Some(state)) => state,
        other => return Err(unexpected(other)),
    };
    let ctx = context(&state.id, state.epoch);
    let confirm = AccountOp::ConfirmMember { epoch: state.epoch, key: member_key(&team_key, &ctx, state.me, public)? };
    request(server, key, confirm).await?;
    let book = TeamBook { name: name.trim().to_string(), book: Book::default() };
    let blob = seal(&team_key, &aad(BOOK_AAD, &state.id, state.epoch), &serde_json::to_vec(&book)?)?;
    request(server, key, AccountOp::PutTeamBook { epoch: state.epoch, base: state.revision, blob }).await?;
    let state = get(server, key).await?.context("Team ist verschwunden")?;
    view(state, team_key)
}

/// The account's team, decrypted; finishes a join whose confirmation got
/// lost and, as an admin, starts the new epoch someone's leaving asks for.
pub async fn sync(server: &str, key: &SigningKey, account: &AccountLink, cached: Option<&TeamLink>) -> Result<Option<TeamView>> {
    let Some(state) = get(server, key).await? else { return Ok(None) };
    let team_key = team_key(account, &state, cached)?;
    if state.sealed.is_none() {
        let ctx = context(&state.id, state.epoch);
        let public = exchange_public(&account.key()?);
        let op = AccountOp::ConfirmMember { epoch: state.epoch, key: member_key(&team_key, &ctx, state.me, public)? };
        request(server, key, op).await?;
    }
    let team = view(state, team_key)?;
    if team.stale && team.link.role.manages() {
        return rotate(server, key, account, &[]).await.map(Some);
    }
    Ok(Some(team))
}

/// Opens an invitation for someone with `role` and returns the code to give
/// them, as `ABCD-EFGH-JKMN`; valid for [`wire::PAIRING_SECS`] and once.
pub async fn invite(server: &str, key: &SigningKey, account: &AccountLink, role: TeamRole) -> Result<String> {
    let state = get(server, key).await?.context("Dieses Konto gehört zu keinem Team")?;
    let team_key = team_key(account, &state, None)?;
    for _ in 0..5 {
        let code: String = (0..CODE_ID_LEN + wire::CODE_SECRET_LEN)
            .map(|_| CODE_ALPHABET[rand::random::<usize>() % CODE_ALPHABET.len()] as char)
            .collect();
        let (code_id, secret) = code.split_at(CODE_ID_LEN);
        let salt: [u8; 16] = rand::random();
        let (seal_key, verifier) = derive(secret, &salt)?;
        let sealed = seal(&seal_key, &aad(INVITE_AAD, &state.id, state.epoch), &team_key)?;
        let op = AccountOp::OfferInvite { code_id: code_id.into(), salt, verifier, sealed, role };
        match request(server, key, op).await {
            Ok(_) => return Ok(format!("{}-{}-{}", &code[..4], &code[4..8], &code[8..])),
            Err(e) if is(&e, AccountError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Kein freier Code gefunden, bitte nochmal versuchen")
}

/// Joins the team whose admin gave out `code`.
pub async fn join(server: &str, key: &SigningKey, account: &AccountLink, code: &str) -> Result<TeamView> {
    let code = wire::normalize_code(code).context("Der Code hat 12 Zeichen, z. B. ABCD-EFGH-JKMN")?;
    let (code_id, secret) = code.split_at(CODE_ID_LEN);
    let salt = match request(server, key, AccountOp::InviteSalt { code_id: code_id.into() }).await? {
        AccountReply::Salt(salt) => salt,
        other => return Err(unexpected(other)),
    };
    let (seal_key, proof) = derive(secret, &salt)?;
    let public = exchange_public(&account.key()?);
    let op = AccountOp::JoinTeam { code_id: code_id.into(), proof, public_key: public };
    let (sealed, state) = match request(server, key, op).await? {
        AccountReply::JoinedTeam { sealed, team } => (sealed, team),
        other => return Err(unexpected(other)),
    };
    let team_key: [u8; 32] = open(&seal_key, &aad(INVITE_AAD, &state.id, state.epoch), &sealed)?
        .try_into()
        .map_err(|_| anyhow!("Teamschlüssel ist beschädigt"))?;
    let ctx = context(&state.id, state.epoch);
    let op = AccountOp::ConfirmMember { epoch: state.epoch, key: member_key(&team_key, &ctx, state.me, public)? };
    request(server, key, op).await?;
    let state = get(server, key).await?.context("Team ist verschwunden")?;
    view(state, team_key)
}

/// Changes the shared address book with `change` (owner, admins); retries
/// on top of what others wrote meanwhile.
pub async fn update_book(
    server: &str,
    key: &SigningKey,
    account: &AccountLink,
    change: impl Fn(&mut TeamBook),
) -> Result<TeamView> {
    for _ in 0..4 {
        let state = get(server, key).await?.context("Dieses Konto gehört zu keinem Team")?;
        let (id, epoch, base) = (state.id, state.epoch, state.revision);
        let team_key = team_key(account, &state, None)?;
        let mut team = view(state, team_key)?;
        change(&mut team.book);
        let blob = seal(&team_key, &aad(BOOK_AAD, &id, epoch), &serde_json::to_vec(&team.book)?)?;
        match request(server, key, AccountOp::PutTeamBook { epoch, base, blob }).await {
            Ok(AccountReply::Stored { revision }) => {
                team.revision = revision;
                return Ok(team);
            }
            Ok(other) => return Err(unexpected(other)),
            Err(e) if is(&e, AccountError::Conflict) || is(&e, AccountError::StaleKey) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Das Team-Adressbuch ändert sich gerade ständig, bitte später nochmal versuchen")
}

/// Replaces this member's list of devices open to the team.
pub async fn set_access(server: &str, key: &SigningKey, account: &AccountLink, list: &AccessList) -> Result<()> {
    for _ in 0..4 {
        let state = get(server, key).await?.context("Dieses Konto gehört zu keinem Team")?;
        let team_key = team_key(account, &state, None)?;
        let blob = seal(&team_key, &access_aad(&state.id, state.epoch, state.me), &serde_json::to_vec(list)?)?;
        match request(server, key, AccountOp::SetTeamAccess { epoch: state.epoch, blob }).await {
            Ok(_) => return Ok(()),
            Err(e) if is(&e, AccountError::StaleKey) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Der Teamschlüssel ändert sich gerade ständig, bitte später nochmal versuchen")
}

pub async fn set_role(server: &str, key: &SigningKey, member: u64, role: TeamRole) -> Result<()> {
    request(server, key, AccountOp::SetRole { member, role }).await.map(|_| ())
}

/// Removes `remove` (may be empty) and moves the team to a new epoch, also
/// dropping members the old key does not vouch for.
pub async fn rotate(server: &str, key: &SigningKey, account: &AccountLink, remove: &[u64]) -> Result<TeamView> {
    for _ in 0..4 {
        let state = get(server, key).await?.context("Dieses Konto gehört zu keinem Team")?;
        let old_key = team_key(account, &state, None)?;
        let (id, epoch, base) = (state.id, state.epoch, state.revision);
        let old = view(state.clone(), old_key)?;
        let mut gone: Vec<u64> = remove.to_vec();
        gone.extend(old.unverified.iter().filter(|m| **m != state.me));
        gone.sort_unstable();
        gone.dedup();

        let new_key: [u8; 32] = rand::random();
        let next = epoch + 1;
        let ctx = context(&id, next);
        let mut members = Vec::new();
        for m in old.members.iter().filter(|m| !gone.contains(&m.id)) {
            let (public, _) = m.key.context("Mitglied ohne Schlüssel")?;
            members.push((m.id, member_key(&new_key, &ctx, m.id, public)?));
        }
        let blob = seal(&new_key, &aad(BOOK_AAD, &id, next), &serde_json::to_vec(&old.book)?)?;
        let mut access = Vec::new();
        for (member, sealed) in &state.access {
            if gone.contains(member) {
                continue;
            }
            if let Ok(plain) = open(&old_key, &access_aad(&id, epoch, *member), sealed) {
                access.push((*member, seal(&new_key, &access_aad(&id, next, *member), &plain)?));
            }
        }
        let rotation = Rotation { epoch: next, base, blob, members, access };
        match request(server, key, AccountOp::Rotate { remove: gone, rotation }).await {
            Ok(AccountReply::Team(Some(state))) => return view(state, new_key),
            Ok(other) => return Err(unexpected(other)),
            Err(e) if is(&e, AccountError::Conflict) || is(&e, AccountError::StaleKey) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Das Team ändert sich gerade ständig, bitte später nochmal versuchen")
}

pub async fn leave(server: &str, key: &SigningKey) -> Result<()> {
    match request(server, key, AccountOp::LeaveTeam).await {
        Ok(_) => Ok(()),
        Err(e) if is(&e, AccountError::NoTeam) => Ok(()),
        Err(e) => Err(e),
    }
}

pub async fn delete(server: &str, key: &SigningKey) -> Result<()> {
    request(server, key, AccountOp::DeleteTeam).await.map(|_| ())
}

/// What a host needs to let its team's members in without a password.
/// Kept in the host's config; for the service handed over elevated once,
/// then renewed with each epoch (see "Zugriff ohne Passwort im Team").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAccessGrant {
    /// The device ID the password is for.
    pub host: DeviceId,
    /// The team (hex) and epoch the password belongs to.
    pub team: String,
    pub epoch: u32,
    /// [`access_password`] for `host`.
    pub password: String,
    /// Hex public key of a member device on this computer (the app's): a
    /// viewer must be in the same team as this key, the server says.
    pub witness: String,
}

impl TeamAccessGrant {
    /// For `host`, from the team of `config` (the app's own config).
    pub fn for_host(config: &crate::config::Config, host: DeviceId) -> Result<Self> {
        let team = config.team.as_ref().context("Dieses Konto gehört zu keinem Team")?;
        Ok(Self {
            host,
            team: team.id.clone(),
            epoch: team.epoch,
            password: access_password(&team.key()?, host),
            witness: hex::encode(config.signing_key()?.verifying_key().to_bytes()),
        })
    }

    pub fn witness_key(&self) -> Option<[u8; 32]> {
        hex::decode(&self.witness).ok()?.try_into().ok()
    }

    /// Whether `next` only renews this grant for a later epoch of the same
    /// team, host and witness; that needs no elevation.
    pub fn renewed_by(&self, next: &Self) -> bool {
        next.host == self.host && next.team == self.team && next.witness == self.witness && next.epoch > self.epoch
    }
}

/// The team's access password for connecting to `target`, if a member said
/// it lets the team in without a password.
pub fn access_for(config: &crate::config::Config, target: DeviceId) -> Option<String> {
    let team = config.team.as_ref()?;
    config.team_open.contains_key(&target.get()).then_some(())?;
    Some(access_password(&team.key().ok()?, target))
}

/// Whether the server counts both device keys as confirmed members of one
/// team. Anything but a clear yes (old server, error) is a no.
pub async fn same_team(server: &str, a: [u8; 32], b: [u8; 32]) -> bool {
    use ctxremote_proto::framing;
    use ctxremote_proto::rendezvous::{ClientMsg, ServerMsg};
    let ask = async {
        let (mut t, _) = crate::net::dial(server).await?;
        framing::send(&mut t, &ClientMsg::SameTeam { a, b }).await?;
        anyhow::Ok(matches!(framing::recv::<ServerMsg>(&mut t).await?, ServerMsg::SameTeam(true)))
    };
    matches!(tokio::time::timeout(std::time::Duration::from_secs(15), ask).await, Ok(Ok(true)))
}

/// Unix milliseconds, for access list entries.
pub fn stamp() -> u64 {
    now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_keys_open_only_for_their_account_and_context() {
        let (alice, bob) = ([1u8; 32], [2u8; 32]);
        let ctx = context(&[7; 16], 3);
        let sealed = seal_to(&exchange_public(&alice), &ctx, b"team key").unwrap();
        assert_eq!(open_with(&exchange_secret(&alice), &ctx, &sealed).unwrap(), b"team key");
        assert!(open_with(&exchange_secret(&bob), &ctx, &sealed).is_err());
        assert!(open_with(&exchange_secret(&alice), &context(&[7; 16], 4), &sealed).is_err());
        // Fresh for every seal.
        assert_ne!(sealed, seal_to(&exchange_public(&alice), &ctx, b"team key").unwrap());
    }

    #[test]
    fn tags_need_the_team_key() {
        let public = exchange_public(&[1; 32]);
        let member = TeamMember { id: 5, role: TeamRole::Technician, email: None, key: Some((public, tag(&[9; 32], 5, &public))), since: 0 };
        assert!(tag_ok(&[9; 32], &member));
        assert!(!tag_ok(&[8; 32], &member));
        assert!(!tag_ok(&[9; 32], &TeamMember { id: 6, ..member.clone() }));
        assert!(!tag_ok(&[9; 32], &TeamMember { key: None, ..member }));
        assert_ne!(access_password(&[9; 32], DeviceId::new(123_456_789).unwrap()), access_password(&[8; 32], DeviceId::new(123_456_789).unwrap()));
    }

    /// The web interface (web/src/lib/crypto.ts, scripts/vectors.mjs) must
    /// give the same results; these values are checked there too.
    #[test]
    fn vectors_stay_stable() {
        let public = exchange_public(&[1; 32]);
        assert_eq!(hex::encode(public), "16ae5d36023369e4763ae8a0ae2b6bab2bec03b4ca2c72d9936079733aebe125");
        assert_eq!(hex::encode(tag(&[9; 32], 5, &public)), "380f0d7c20be9e5f0db88b309f6060561e8be16e00edb0568fdcbbc0bd15799f");
        assert_eq!(
            access_password(&[9; 32], DeviceId::new(123_456_789).unwrap()),
            "c2fbab987d58ab1b2eec0a1c9c8d24b7e10c2772a9496694d4ec57c12ed7f541"
        );
        let ctx = context(&[7; 16], 3);
        let sealed = hex::decode("ab3db33ad7f1bca84cb50c22cc91ffaf2ca5121e9b134f211d1489d5b597591a9de4b6d8cbd87723503747a7cc29ab881a1afc178c39c15247820de2bc57f2471cbad5abec32e3cc9434794f4d7af516638087c9c8517c5dce346805").unwrap();
        assert_eq!(open_with(&exchange_secret(&[1; 32]), &ctx, &sealed).unwrap(), [9; 32]);
        let book = hex::decode("6d52a2542760ef9708a4f728b8f5a47e9f71d3d566ae9ad652c02f73d1723896a961a7309b0ba4a6aa2ce141d2b1764e356b29a79db1047fefa40e6cb0f8f9f217280ebafa8c80994c4fc8730803d0b3a45417").unwrap();
        assert_eq!(
            open(&[9; 32], &aad(BOOK_AAD, &[7; 16], 3), &book).unwrap(),
            br#"{"name":"Werkstatt","book":{"entries":{},"removed":{}}}"#
        );
        let list = hex::decode("2b07cde8b3253f9d3f463811823a4790623d53a611c151f7f9d1f634376bd98c267f998fa97bfab2e75013167cd4620a2763cbf0ea253c8a7bb419e87a69c63488").unwrap();
        assert_eq!(open(&[9; 32], &access_aad(&[7; 16], 3, 5), &list).unwrap(), br#"{"hosts":{"123456789":["Empfang",1]}}"#);
        let invite = hex::decode("b6b26c564ca21849b807b7ec218151220e485e2c96cef16fc5fb7318a0a4565a99dabd4ecfc16eca07e3e862c989778bdfde63c9b1d01eefdd6a2216").unwrap();
        assert_eq!(open(&[4; 32], &aad(INVITE_AAD, &[7; 16], 3), &invite).unwrap(), [9; 32]);
    }
}
