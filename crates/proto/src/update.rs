//! Client updates, served by the rendezvous server (see `docs/DEPLOY.md`).
//!
//! The server only stores and hands out what the release pipeline put into
//! its data folder. Trust comes from an Ed25519 signature made in the
//! pipeline: clients carry the public key and install nothing that does not
//! verify, so a compromised server cannot push its own installer. Clients
//! also never install a version that is not newer than their own.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Bytes per [`crate::rendezvous::ServerMsg::UpdateData`] frame.
pub const CHUNK: usize = 1024 * 1024;

/// An available update as the server announces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// E.g. `windows-x86_64`; see `PLATFORM` in the client.
    pub platform: String,
    pub version: String,
    pub size: u64,
    pub sha256: [u8; 32],
    /// Ed25519 over [`signed_payload`].
    pub signature: Vec<u8>,
    /// Shown to the user; not signed, so never trusted for anything else.
    pub notes: String,
}

fn signed_payload(platform: &str, version: &str, size: u64, sha256: &[u8; 32]) -> Vec<u8> {
    let mut payload = b"ctxremote/update/v1\0".to_vec();
    for part in [platform.as_bytes(), version.as_bytes()] {
        payload.extend_from_slice(part);
        payload.push(0);
    }
    payload.extend_from_slice(&size.to_le_bytes());
    payload.extend_from_slice(sha256);
    payload
}

impl UpdateInfo {
    /// Signs an installer's metadata; used by the release pipeline.
    pub fn sign(key: &SigningKey, platform: &str, version: &str, size: u64, sha256: [u8; 32], notes: String) -> Self {
        let signature = key.sign(&signed_payload(platform, version, size, &sha256)).to_bytes().to_vec();
        Self { platform: platform.into(), version: version.into(), size, sha256, signature, notes }
    }

    /// Whether the metadata (and so the file hash) was signed with `public_key`.
    pub fn verify(&self, public_key: &[u8; 32]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(public_key) else { return false };
        let Ok(signature) = Signature::from_slice(&self.signature) else { return false };
        key.verify(&signed_payload(&self.platform, &self.version, self.size, &self.sha256), &signature).is_ok()
    }
}

/// A plain `major.minor.patch` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl FromStr for Version {
    type Err = InvalidVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>());
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(Ok(a)), Some(Ok(b)), Some(Ok(c)), None) => Ok(Self(a, b, c)),
            _ => Err(InvalidVersion),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("ungültige Versionsnummer")]
pub struct InvalidVersion;

/// Whether `offered` is a real step up from `current`; unparsable versions never are.
pub fn is_newer(offered: &str, current: &str) -> bool {
    match (offered.parse::<Version>(), current.parse::<Version>()) {
        (Ok(offered), Ok(current)) => offered.cmp(&current) == Ordering::Greater,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_covers_everything_that_matters() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = key.verifying_key().to_bytes();
        let info = UpdateInfo::sign(&key, "windows-x86_64", "0.1.42", 1234, [9; 32], "Neu: Chat".into());
        assert!(info.verify(&public));

        let tampered = [
            UpdateInfo { version: "0.1.43".into(), ..info.clone() },
            UpdateInfo { platform: "linux".into(), ..info.clone() },
            UpdateInfo { size: 1235, ..info.clone() },
            UpdateInfo { sha256: [8; 32], ..info.clone() },
        ];
        for t in tampered {
            assert!(!t.verify(&public));
        }
        // Notes are display-only and may change without breaking anything.
        assert!(UpdateInfo { notes: "anders".into(), ..info.clone() }.verify(&public));
        assert!(!info.verify(&SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes()));
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.99"));
        assert!(!is_newer("0.1.9", "0.1.9"));
        assert!(!is_newer("0.1.8", "0.1.9"));
        assert!(!is_newer("0.2", "0.1.0"));
        assert!(!is_newer("0.2.0-beta", "0.1.0"));
        assert_eq!("v1.2.3".parse::<Version>().unwrap().to_string(), "1.2.3");
    }
}
