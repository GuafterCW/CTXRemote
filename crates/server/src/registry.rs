//! Persistent mapping of device IDs to the public key that owns them.
//!
//! An ID belongs to the first key that registers it, so nobody can take over
//! a known device's address without its private key.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use ctxremote_proto::rendezvous::{normalize_alias, AliasError};
use ctxremote_proto::DeviceId;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    /// Device ID → hex-encoded Ed25519 public key.
    devices: BTreeMap<u32, String>,
    /// Public alias (normalised) → device ID; at most one per device.
    #[serde(default)]
    aliases: BTreeMap<String, u32>,
}

pub struct Registry {
    path: PathBuf,
    stored: Stored,
}

impl Registry {
    pub fn open(path: PathBuf) -> Result<Self> {
        let stored = match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json)
                .with_context(|| format!("{} ist beschädigt", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, stored })
    }

    /// Returns the ID this key may use: the requested one if it is free or
    /// already theirs, else the key's previous ID, else a fresh one.
    pub fn assign(&mut self, requested: Option<DeviceId>, public_key: [u8; 32]) -> Result<DeviceId> {
        let key = hex::encode(public_key);
        if let Some(id) = requested {
            match self.stored.devices.get(&id.get()) {
                Some(owner) if *owner == key => return Ok(id),
                None => return self.insert(id, key),
                Some(_) => {}
            }
        }
        if let Some((&raw, _)) = self.stored.devices.iter().find(|(_, owner)| **owner == key) {
            return Ok(DeviceId::new(raw).expect("stored IDs are valid"));
        }
        let id = loop {
            let candidate = DeviceId::random();
            if !self.stored.devices.contains_key(&candidate.get()) {
                break candidate;
            }
        };
        self.insert(id, key)
    }

    /// Sets or drops the public alias of `id`, if `public_key` owns it.
    /// Returns the alias as stored.
    pub fn claim_alias(
        &mut self,
        id: DeviceId,
        public_key: [u8; 32],
        alias: Option<&str>,
    ) -> Result<Option<String>, AliasError> {
        if self.stored.devices.get(&id.get()) != Some(&hex::encode(public_key)) {
            return Err(AliasError::UnknownDevice);
        }
        let alias = alias.map(normalize_alias).transpose()?;
        if let Some(alias) = &alias {
            if self.stored.aliases.get(alias).is_some_and(|owner| *owner != id.get()) {
                return Err(AliasError::Taken);
            }
        }
        let before = self.stored.aliases.clone();
        self.stored.aliases.retain(|_, owner| *owner != id.get());
        if let Some(alias) = &alias {
            self.stored.aliases.insert(alias.clone(), id.get());
        }
        if self.stored.aliases != before {
            // A failed write must not leave memory and disk disagreeing.
            if self.save().is_err() {
                self.stored.aliases = before;
                return Err(AliasError::UnknownDevice);
            }
        }
        Ok(alias)
    }

    pub fn resolve_alias(&self, alias: &str) -> Option<DeviceId> {
        let alias = normalize_alias(alias).ok()?;
        self.stored.aliases.get(&alias).and_then(|raw| DeviceId::new(*raw))
    }

    fn insert(&mut self, id: DeviceId, key: String) -> Result<DeviceId> {
        self.stored.devices.insert(id.get(), key);
        self.save()?;
        Ok(id)
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.stored)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_belong_to_one_device() {
        let path = std::env::temp_dir().join(format!("ctxremote-registry-{}.json", rand::random::<u32>()));
        let mut registry = Registry::open(path.clone()).unwrap();
        let (a_key, b_key) = ([1; 32], [2; 32]);
        let a = registry.assign(None, a_key).unwrap();
        let b = registry.assign(None, b_key).unwrap();

        assert_eq!(registry.claim_alias(a, a_key, Some("Philipp-PC")).unwrap().as_deref(), Some("philipp-pc"));
        assert_eq!(registry.resolve_alias("PHILIPP-pc"), Some(a));
        assert_eq!(registry.claim_alias(b, b_key, Some("philipp-pc")), Err(AliasError::Taken));
        // Someone else's key cannot claim for a's ID.
        assert_eq!(registry.claim_alias(a, b_key, Some("x-y-z")), Err(AliasError::UnknownDevice));
        assert_eq!(registry.claim_alias(a, a_key, Some("1234")), Err(AliasError::Invalid));

        // Renaming frees the old alias; it survives a restart.
        registry.claim_alias(a, a_key, Some("buero")).unwrap();
        assert_eq!(registry.resolve_alias("philipp-pc"), None);
        let reopened = Registry::open(path.clone()).unwrap();
        assert_eq!(reopened.resolve_alias("buero"), Some(a));

        registry.claim_alias(a, a_key, None).unwrap();
        assert_eq!(registry.resolve_alias("buero"), None);
        std::fs::remove_file(path).unwrap();
    }
}
