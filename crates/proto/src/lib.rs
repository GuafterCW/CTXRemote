//! Wire protocol shared by the CTXRemote server, host and viewer.
//!
//! Every connection starts at the rendezvous server ([`rendezvous`]). Once a
//! viewer and a host have been paired, the server relays raw bytes and the two
//! peers run a password-authenticated handshake ([`secure`]) on top, so the
//! server never sees session content.

pub mod framing;
pub mod rendezvous;
pub mod secure;
pub mod session;
pub mod update;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Bumped whenever a message layout changes incompatibly.
pub const PROTOCOL_VERSION: u16 = 1;

/// Default TCP port of `ctxremote-server`.
pub const DEFAULT_PORT: u16 = 21300;

/// Nine-digit public address of a device, e.g. `482 913 077`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(u32);

impl DeviceId {
    pub const MIN: u32 = 100_000_000;
    pub const MAX: u32 = 999_999_999;

    pub fn new(raw: u32) -> Option<Self> {
        (Self::MIN..=Self::MAX).contains(&raw).then_some(Self(raw))
    }

    pub fn random() -> Self {
        use rand::Rng;
        Self(rand::thread_rng().gen_range(Self::MIN..=Self::MAX))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0;
        write!(f, "{:03} {:03} {:03}", n / 1_000_000, n / 1_000 % 1_000, n % 1_000)
    }
}

impl FromStr for DeviceId {
    type Err = InvalidDeviceId;

    /// Accepts any grouping of the nine digits ("482913077", "482 913 077", "482-913-077").
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits: String = s.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        if digits.len() != 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidDeviceId);
        }
        digits.parse().ok().and_then(Self::new).ok_or(InvalidDeviceId)
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("eine ID besteht aus neun Ziffern")]
pub struct InvalidDeviceId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_roundtrip() {
        let id: DeviceId = "482-913 077".parse().unwrap();
        assert_eq!(id.get(), 482_913_077);
        assert_eq!(id.to_string(), "482 913 077");
        assert!("12345678".parse::<DeviceId>().is_err());
        assert!("012345678".parse::<DeviceId>().is_err());
    }
}
