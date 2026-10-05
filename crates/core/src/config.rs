//! Persistent per-user settings and device identity.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use ctxremote_proto::{DeviceId, DEFAULT_PORT};
use ed25519_dalek::SigningKey;
use rand::Rng;
use ctxremote_proto::session::Permissions;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `host` or `host:port` of the rendezvous server.
    pub server: String,
    /// The ID the server assigned last time; we ask to keep it.
    pub device_id: Option<DeviceId>,
    /// Hex-encoded Ed25519 secret key that owns `device_id`.
    pub device_key: String,
    /// Enables unattended access when set.
    pub permanent_password: Option<String>,
    /// Known devices: everything with an alias, plus the most recent connections.
    #[serde(alias = "recent")]
    pub peers: Vec<Peer>,
    /// Offers viewers a direct TCP connection to this device (see `docs/DIRECT.md`).
    pub direct: bool,
    /// Port of the direct listener; forward it on the router for direct
    /// connections from the internet.
    pub direct_port: u16,
    /// Extra `host:port` addresses offered to viewers, e.g. the router's public
    /// name when the port is forwarded.
    pub direct_addresses: Vec<String>,
    /// Public alias others can connect with instead of the ID (kept on the server).
    pub public_alias: Option<String>,
    /// Whether `direct` includes the TCP listener. Without it (the quick
    /// helper, which must not open a port) only the UDP path through NAT is
    /// offered, which needs no listening port.
    pub direct_listen: bool,
    /// How this user presents themselves when connecting to others.
    pub profile: Option<crate::profile::Profile>,
    /// Devices removed from the list, with Unix milliseconds, so the account's
    /// address book does not bring them back (see `crate::account`).
    pub removed: Vec<(DeviceId, u64)>,
    /// Set while this user's address book syncs with an account.
    pub account: Option<crate::account::AccountLink>,
    /// Which devices let the account's devices in without a password (from
    /// the account's book; device ID → choice).
    pub access: std::collections::BTreeMap<u32, crate::account::Access>,
    /// Set when this device lets the account's devices in without a password.
    pub account_access: Option<crate::account::AccessGrant>,
    /// What viewers may do in sessions opened with the one-time password,
    /// i.e. with someone at the computer. Changeable per session.
    pub rights_attended: Permissions,
    /// The same for the permanent password and the account's devices.
    pub rights_unattended: Permissions,
    /// Secret of the authenticator app (base32); with it, the permanent
    /// password also needs the current code (see [`crate::totp`]).
    pub code_secret: Option<String>,
    /// Devices whose screen this viewer locks when it ends a session there.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lock_on_end: Vec<DeviceId>,
}

/// The direct-connection part of the settings form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectSettings {
    pub enabled: bool,
    pub port: u16,
    pub addresses: Vec<String>,
}

const MAX_DIRECT_ADDRESSES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: DeviceId,
    /// The user's own name for the device; shown instead of the ID.
    #[serde(default)]
    pub alias: Option<String>,
    /// Hostname reported by the device on the last connection.
    pub name: String,
    /// Unix seconds of the last successful connection, 0 if never connected.
    pub last_seen: u64,
    /// Unix milliseconds of the last alias change, for merging with the
    /// account's address book; 0 if never set here.
    #[serde(default)]
    pub alias_at: u64,
    /// MAC addresses of its network cards, learned from its system info.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macs: Vec<String>,
    /// The user's groups for the device, e.g. a customer or a site.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Unix milliseconds of the last change to `tags`; the newer wins.
    #[serde(default)]
    pub tags_at: u64,
}

impl Peer {
    pub fn label(&self) -> &str {
        match (&self.alias, self.name.as_str()) {
            (Some(alias), _) => alias,
            (None, "") => "Unbenanntes Gerät",
            (None, name) => name,
        }
    }
}

/// Peers without an alias are a history and only the newest few are kept.
const MAX_UNNAMED_PEERS: usize = 12;
const MAX_ALIAS_LEN: usize = 40;
const MAX_TAG_LEN: usize = 24;
const MAX_TAGS: usize = 8;

impl Default for Config {
    fn default() -> Self {
        Self {
            // Release builds point at the project's server (see docs/DEPLOY.md).
            server: option_env!("CTXREMOTE_DEFAULT_SERVER").map_or(format!("localhost:{DEFAULT_PORT}"), str::to_string),
            device_id: None,
            device_key: hex::encode(SigningKey::generate(&mut rand::rngs::OsRng).to_bytes()),
            permanent_password: None,
            peers: Vec::new(),
            direct: true,
            direct_port: crate::direct::DEFAULT_PORT,
            direct_addresses: Vec::new(),
            public_alias: None,
            direct_listen: true,
            profile: None,
            removed: Vec::new(),
            account: None,
            access: Default::default(),
            account_access: None,
            rights_attended: Permissions::ATTENDED,
            rights_unattended: Permissions::ALL,
            code_secret: None,
            lock_on_end: Vec::new(),
        }
    }
}

/// Set once per process, e.g. to the service's machine-wide config.
static PATH: OnceLock<PathBuf> = OnceLock::new();

impl Config {
    /// Pins the config file for this process. Only the first call has an effect.
    pub fn use_path(path: PathBuf) {
        let _ = PATH.set(path);
    }

    /// The path pinned with [`Config::use_path`], else `CTXREMOTE_CONFIG` (useful for
    /// a second instance on one machine), else
    /// `%APPDATA%\philipp-dev\CTXRemote\config\config.json`.
    pub fn path() -> Result<PathBuf> {
        if let Some(path) = PATH.get() {
            return Ok(path.clone());
        }
        if let Some(path) = std::env::var_os("CTXREMOTE_CONFIG") {
            return Ok(path.into());
        }
        let dirs = directories::ProjectDirs::from("info", "philipp-dev", "CTXRemote")
            .context("kein Benutzerverzeichnis gefunden")?;
        Ok(dirs.config_dir().join("config.json"))
    }

    /// Loads the config, creating and saving a fresh identity on first start.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_else(|e| {
                // Starting over means a new key and thus a new device ID; keep the
                // old file so the identity can be restored by hand.
                let aside = path.with_extension(format!("json.broken-{}", crate::account::now_ms() / 1000));
                let kept = std::fs::copy(&path, &aside).is_ok();
                tracing::error!(
                    "Konfiguration {} unlesbar ({e}), starte mit neuer Identität{}",
                    path.display(),
                    if kept { format!("; alte Datei liegt in {}", aside.display()) } else { String::new() }
                );
                Self::default()
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Self::default();
                config.save()?;
                Ok(config)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        std::fs::create_dir_all(path.parent().expect("config path has a parent"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn signing_key(&self) -> Result<SigningKey> {
        let bytes: [u8; 32] = hex::decode(&self.device_key)
            .ok()
            .and_then(|b| b.try_into().ok())
            .context("Geräteschlüssel ist beschädigt")?;
        Ok(SigningKey::from_bytes(&bytes))
    }

    /// Applies the settings form. `permanent_password`: `None` keeps the current
    /// one, `Some("")` disables unattended access. Returns whether the server changed.
    pub fn apply_settings(&mut self, server: &str, permanent_password: Option<&str>) -> Result<bool> {
        let server = server.trim();
        if server.is_empty() {
            bail!("Bitte eine Serveradresse angeben");
        }
        if let Some(password) = permanent_password {
            if !password.is_empty() && password.chars().count() < 8 {
                bail!("Das Passwort braucht mindestens 8 Zeichen");
            }
            self.permanent_password = Some(password.to_string()).filter(|p| !p.is_empty());
        }
        let changed = self.server != server;
        self.server = server.to_string();
        Ok(changed)
    }

    pub fn direct_settings(&self) -> DirectSettings {
        DirectSettings {
            enabled: self.direct,
            port: self.direct_port,
            addresses: self.direct_addresses.clone(),
        }
    }

    /// Validates and applies the direct-connection form. Returns whether anything changed.
    pub fn apply_direct(&mut self, s: &DirectSettings) -> Result<bool> {
        if s.port < 1024 {
            bail!("Der Port muss zwischen 1024 und 65535 liegen");
        }
        let addresses: Vec<String> = s
            .addresses
            .iter()
            .map(|a| a.trim())
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .collect();
        if addresses.len() > MAX_DIRECT_ADDRESSES {
            bail!("Es sind höchstens {MAX_DIRECT_ADDRESSES} zusätzliche Adressen möglich");
        }
        for address in &addresses {
            if !valid_host_port(address) {
                bail!("„{address}“ ist keine gültige Adresse (Format: host:port)");
            }
        }
        let new = DirectSettings { enabled: s.enabled, port: s.port, addresses };
        let changed = self.direct_settings() != new;
        self.direct = new.enabled;
        self.direct_port = new.port;
        self.direct_addresses = new.addresses;
        Ok(changed)
    }

    pub fn server_addr(&self) -> String {
        let server = self.server.trim();
        if server.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
            server.to_string()
        } else {
            format!("{server}:{DEFAULT_PORT}")
        }
    }

    pub fn peer(&self, id: DeviceId) -> Option<&Peer> {
        self.peers.iter().find(|p| p.id == id)
    }

    /// Records a successful connection, keeping any alias.
    pub fn remember(&mut self, id: DeviceId, name: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // Read before the entry is taken out, or the alias's age would be lost.
        let alias = self.peer(id).and_then(|p| p.alias.clone());
        let alias_at = self.peer(id).map_or(0, |p| p.alias_at);
        let macs = self.peer(id).map(|p| p.macs.clone()).unwrap_or_default();
        let (tags, tags_at) = self.peer(id).map(|p| (p.tags.clone(), p.tags_at)).unwrap_or_default();
        self.peers.retain(|p| p.id != id);
        self.peers.insert(0, Peer { id, alias, name: name.to_string(), last_seen: now, alias_at, macs, tags, tags_at });
        self.prune();
    }

    /// Sets or clears (`None` / blank) the alias, adding the device if it is new.
    pub fn set_alias(&mut self, id: DeviceId, alias: Option<&str>) -> Result<()> {
        let alias = alias.map(str::trim).filter(|a| !a.is_empty());
        if let Some(alias) = alias {
            if alias.chars().count() > MAX_ALIAS_LEN {
                bail!("Ein Alias darf höchstens {MAX_ALIAS_LEN} Zeichen haben");
            }
            if alias.parse::<DeviceId>().is_ok() {
                bail!("Ein Alias darf keine ID sein");
            }
            let taken = self.peers.iter().any(|p| {
                p.id != id && p.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(alias))
            });
            if taken {
                bail!("„{alias}“ wird bereits für ein anderes Gerät verwendet");
            }
        }
        let alias_at = crate::account::now_ms();
        match self.peers.iter_mut().find(|p| p.id == id) {
            Some(peer) => {
                peer.alias = alias.map(Into::into);
                peer.alias_at = alias_at;
            }
            None => self.peers.push(Peer {
                id,
                alias: alias.map(Into::into),
                name: String::new(),
                last_seen: 0,
                alias_at,
                macs: Vec::new(),
                tags: Vec::new(),
                tags_at: 0,
            }),
        }
        self.prune();
        Ok(())
    }

    /// Sets the groups of a known device (trimmed, without doubles).
    pub fn set_tags(&mut self, id: DeviceId, tags: &[String]) -> Result<()> {
        let mut clean: Vec<String> = Vec::new();
        for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            if tag.chars().count() > MAX_TAG_LEN {
                bail!("Ein Gruppenname darf höchstens {MAX_TAG_LEN} Zeichen haben");
            }
            if !clean.iter().any(|c| c.eq_ignore_ascii_case(tag)) {
                clean.push(tag.to_string());
            }
        }
        if clean.len() > MAX_TAGS {
            bail!("Ein Gerät kann in höchstens {MAX_TAGS} Gruppen sein");
        }
        let peer = self.peers.iter_mut().find(|p| p.id == id).context("Gerät nicht in der Liste")?;
        peer.tags = clean;
        peer.tags_at = crate::account::now_ms();
        Ok(())
    }

    /// The network cards of a known device, for waking it ([`crate::wol`]).
    /// Returns whether they changed.
    pub fn set_macs(&mut self, id: DeviceId, mut macs: Vec<String>) -> bool {
        macs.sort();
        macs.dedup();
        match self.peers.iter_mut().find(|p| p.id == id) {
            Some(peer) if !macs.is_empty() && peer.macs != macs => {
                peer.macs = macs;
                true
            }
            _ => false,
        }
    }

    pub fn forget(&mut self, id: DeviceId) {
        self.peers.retain(|p| p.id != id);
        self.removed.retain(|(r, _)| *r != id);
        self.removed.push((id, crate::account::now_ms()));
    }

    /// Accepts an ID in any grouping or an exact alias (case-insensitive).
    pub fn resolve(&self, input: &str) -> Option<DeviceId> {
        let input = input.trim();
        input.parse().ok().or_else(|| {
            self.peers
                .iter()
                .find(|p| p.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(input)))
                .map(|p| p.id)
        })
    }

    pub(crate) fn prune(&mut self) {
        // Never-connected peers only exist because of their alias.
        self.peers.retain(|p| p.alias.is_some() || p.last_seen > 0);
        let mut unnamed = 0;
        self.peers.retain(|p| {
            if p.alias.is_some() {
                return true;
            }
            unnamed += 1;
            unnamed <= MAX_UNNAMED_PEERS
        });
    }
}

/// `host:port` with a numeric port 1-65535; IPv6 literals need brackets.
fn valid_host_port(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else { return false };
    if !port.parse::<u16>().is_ok_and(|p| p != 0) {
        return false;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    !host.is_empty() && !host.contains(|c: char| c.is_whitespace() || c == ':' || c == '[' || c == ']')
}

/// A one-time password that is easy to read aloud: no 0/O, 1/l/I.
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..8).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> DeviceId {
        DeviceId::new(n).unwrap()
    }

    #[test]
    fn aliases_resolve_and_survive_reconnects() {
        let mut config = Config::default();
        config.remember(id(111_111_111), "PC-01");
        config.set_alias(id(111_111_111), Some("  Büro ")).unwrap();
        config.remember(id(111_111_111), "PC-01-neu");

        let peer = config.peer(id(111_111_111)).unwrap();
        assert_eq!(peer.alias.as_deref(), Some("Büro"));
        assert_eq!(peer.name, "PC-01-neu");
        assert_eq!(config.resolve("büro"), Some(id(111_111_111)));
        assert_eq!(config.resolve("222 222 222"), Some(id(222_222_222)));
        assert_eq!(config.resolve("Keller"), None);
    }

    #[test]
    fn alias_rules() {
        let mut config = Config::default();
        config.set_alias(id(111_111_111), Some("NAS")).unwrap();
        assert!(config.set_alias(id(222_222_222), Some("nas")).is_err());
        assert!(config.set_alias(id(222_222_222), Some("333 333 333")).is_err());
        // Clearing the alias of a never-connected device removes it again.
        config.set_alias(id(111_111_111), None).unwrap();
        assert!(config.peers.is_empty());
    }

    fn direct(enabled: bool, port: u16, addresses: &[&str]) -> DirectSettings {
        DirectSettings { enabled, port, addresses: addresses.iter().map(|a| a.to_string()).collect() }
    }

    #[test]
    fn direct_settings_apply_and_report_changes() {
        let mut config = Config::default();
        assert!(!config.apply_direct(&config.direct_settings()).unwrap());
        let s = direct(false, 4000, &["  a.example.org:21301 ", "", "[::1]:5000", "1.2.3.4:1"]);
        assert!(config.apply_direct(&s).unwrap());
        assert!(!config.direct);
        assert_eq!(config.direct_port, 4000);
        assert_eq!(config.direct_addresses, ["a.example.org:21301", "[::1]:5000", "1.2.3.4:1"]);
        assert!(!config.apply_direct(&config.direct_settings()).unwrap());
    }

    #[test]
    fn direct_settings_are_validated() {
        let mut config = Config::default();
        let before = config.direct_settings();
        assert!(config.apply_direct(&direct(true, 1023, &[])).is_err());
        assert!(config.apply_direct(&direct(true, 1024, &[])).is_ok());
        for bad in ["host", "host:", "host:0", "host:70000", "ho st:1", ":1", "::1:5", "[::1]", "[x]:5", "a:b"] {
            assert!(config.apply_direct(&direct(true, 21301, &[bad])).is_err(), "{bad}");
        }
        let nine: Vec<&str> = vec!["a.de:1"; 9];
        assert!(config.apply_direct(&direct(true, 21301, &nine)).is_err());
        assert!(config.apply_direct(&direct(true, 21301, &nine[..8])).is_ok());
        // A rejected form leaves the config untouched.
        let mut other = Config::default();
        let _ = other.apply_direct(&direct(false, 5000, &["bad"]));
        assert_eq!(other.direct_settings(), before);
    }

    #[test]
    fn history_is_capped_but_aliases_are_kept() {
        let mut config = Config::default();
        config.set_alias(id(100_000_000), Some("Fest")).unwrap();
        for n in 0..20 {
            config.remember(id(200_000_000 + n), "x");
        }
        assert_eq!(config.peers.len(), MAX_UNNAMED_PEERS + 1);
        assert!(config.peer(id(100_000_000)).is_some());
    }

    #[test]
    fn remembering_keeps_alias_age_and_macs() {
        let mut config = Config::default();
        let id = DeviceId::new(123_456_789).unwrap();
        config.set_alias(id, Some("Büro")).unwrap();
        let at = config.peer(id).unwrap().alias_at;
        assert!(config.set_macs(id, vec!["01:23:45:67:89:ab".into()]));
        assert!(!config.set_macs(id, vec!["01:23:45:67:89:ab".into()]), "unverändert");
        config.remember(id, "PC");
        let peer = config.peer(id).unwrap();
        assert_eq!(peer.alias_at, at);
        assert_eq!(peer.macs.len(), 1);
    }
}