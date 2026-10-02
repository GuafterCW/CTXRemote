//! Persistent mapping of device IDs to the public key that owns them.
//!
//! An ID belongs to the first key that registers it, so nobody can take over
//! a known device's address without its private key.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use ctxremote_proto::DeviceId;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    /// Device ID → hex-encoded Ed25519 public key.
    devices: BTreeMap<u32, String>,
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

    fn insert(&mut self, id: DeviceId, key: String) -> Result<DeviceId> {
        self.stored.devices.insert(id.get(), key);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.stored)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(id)
    }
}
