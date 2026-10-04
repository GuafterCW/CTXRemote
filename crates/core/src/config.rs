//! Persistent per-user settings and device identity.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use ctxremote_proto::{DeviceId, DEFAULT_PORT};
use ed25519_dalek::SigningKey;
use rand::Rng;
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
}

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

impl Default for Config {
    fn default() -> Self {
        Self {
            server: format!("localhost:{DEFAULT_PORT}"),
            device_id: None,
            device_key: hex::encode(SigningKey::generate(&mut rand::rngs::OsRng).to_bytes()),
            permanent_password: None,
            peers: Vec::new(),
            direct: true,
            direct_port: crate::direct::DEFAULT_PORT,
            direct_addresses: Vec::new(),
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
                tracing::warn!("Konfiguration unlesbar, starte neu: {e}");
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
        let alias = self.peer(id).and_then(|p| p.alias.clone());
        self.peers.retain(|p| p.id != id);
        self.peers.insert(0, Peer { id, alias, name: name.to_string(), last_seen: now });
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
        match self.peers.iter_mut().find(|p| p.id == id) {
            Some(peer) => peer.alias = alias.map(Into::into),
            None => self.peers.push(Peer {
                id,
                alias: alias.map(Into::into),
                name: String::new(),
                last_seen: 0,
            }),
        }
        self.prune();
        Ok(())
    }

    pub fn forget(&mut self, id: DeviceId) {
        self.peers.retain(|p| p.id != id);
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

    fn prune(&mut self) {
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
}
