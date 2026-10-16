//! Invitation links: the helper's app hands out a link with a random token;
//! the quick helper downloaded through it knows the token from its file name
//! and sends its ID and one-time password back, sealed with that token.
//!
//! The token never reaches the server: it lives in the link's fragment and
//! in the downloaded file's name. The server only forwards the sealed offer,
//! with a short hint so the app finds the right token, and cannot read the
//! password; end-to-end encryption of the session stays intact.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::DeviceId;

/// Letters of a token: no 0/o, 1/l, which are easy to mix up.
pub const ALPHABET: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";
/// 20 letters of 5 bits: 100 bits.
pub const TOKEN_LEN: usize = 20;
/// Longest password and name an offer carries.
pub const MAX_FIELD: usize = 64;
/// Longest sealed offer the server forwards.
pub const MAX_SEALED: usize = 512;

/// What the quick helper sends to whoever invited it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpOffer {
    /// The helper's own ID; must match what the server says it came from.
    pub from: DeviceId,
    pub password: String,
    /// Who asks, e.g. the computer's user name.
    pub name: String,
}

pub fn new_token() -> String {
    let mut rng = rand::thread_rng();
    (0..TOKEN_LEN).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

pub fn valid_token(token: &str) -> bool {
    token.len() == TOKEN_LEN && token.bytes().all(|b| ALPHABET.contains(&b))
}

/// Lets the app pick the token without trying all of them; tells the server nothing.
pub fn hint(token: &str) -> [u8; 4] {
    let digest = Sha256::new().chain_update(b"ctxremote/invite/hint:").chain_update(token.as_bytes()).finalize();
    [digest[0], digest[1], digest[2], digest[3]]
}

fn cipher(token: &str) -> ChaCha20Poly1305 {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"ctxremote/invite/v1"), token.as_bytes())
        .expand(b"offer", &mut key)
        .expect("32 bytes fit");
    ChaCha20Poly1305::new(Key::from_slice(&key))
}

/// Seals `offer` for whoever holds `token`: a random nonce, then the ciphertext.
pub fn seal(token: &str, offer: &HelpOffer) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill(&mut nonce);
    let plain = postcard::to_stdvec(offer).expect("serializable");
    let sealed = cipher(token).encrypt(Nonce::from_slice(&nonce), plain.as_slice()).expect("encryption cannot fail");
    [nonce.as_slice(), &sealed].concat()
}

/// Opens an offer the server says came from `from`; `None` if the token does
/// not fit, it was changed, or it names another device.
pub fn open(token: &str, from: DeviceId, sealed: &[u8]) -> Option<HelpOffer> {
    if sealed.len() < 12 || sealed.len() > MAX_SEALED {
        return None;
    }
    let (nonce, body) = sealed.split_at(12);
    let plain = cipher(token).decrypt(Nonce::from_slice(nonce), body).ok()?;
    let offer: HelpOffer = postcard::from_bytes(&plain).ok()?;
    let fits = offer.from == from && offer.password.len() <= MAX_FIELD && offer.name.len() <= MAX_FIELD;
    fits.then_some(offer)
}

/// The inviting device and token from a quick helper's file name, as the
/// download page names it: `CTXRemote-Hilfe-<ID>-<token>.exe`. Browsers may
/// add " (1)" and the like; everything after the token is ignored.
pub fn from_file_name(name: &str) -> Option<(DeviceId, String)> {
    let lower = name.to_ascii_lowercase();
    let rest = &lower[lower.find("ctxremote-hilfe-")? + "ctxremote-hilfe-".len()..];
    let (id, rest) = rest.split_at_checked(9)?;
    let id: DeviceId = id.parse().ok()?;
    let token = rest.strip_prefix('-')?.get(..TOKEN_LEN)?;
    valid_token(token).then(|| (id, token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> HelpOffer {
        HelpOffer { from: DeviceId::new(482_913_077).unwrap(), password: "k3x9pq7m".into(), name: "anna".into() }
    }

    #[test]
    fn only_the_token_opens_an_offer() {
        let token = new_token();
        assert!(valid_token(&token));
        let sealed = seal(&token, &offer());
        assert_eq!(open(&token, offer().from, &sealed), Some(offer()));
        assert_eq!(open(&new_token(), offer().from, &sealed), None, "another token");
        let other = DeviceId::new(615_220_948).unwrap();
        assert_eq!(open(&token, other, &sealed), None, "the server swapped the sender");
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(open(&token, offer().from, &changed), None, "changed on the way");
        assert_eq!(hint(&token), hint(&token));
        assert_ne!(hint(&token), hint(&new_token()));
    }

    #[test]
    fn file_names() {
        let token = "abcdefghijkmnpqrst234";
        let token = &token[..TOKEN_LEN];
        let id = DeviceId::new(482_913_077).unwrap();
        assert_eq!(from_file_name(&format!("CTXRemote-Hilfe-482913077-{token}.exe")), Some((id, token.to_string())));
        assert_eq!(from_file_name(&format!("C:\\Users\\a\\Downloads\\CTXRemote-Hilfe-482913077-{token} (1).exe")), Some((id, token.to_string())));
        assert_eq!(from_file_name("CTXRemote-Hilfe.exe"), None);
        assert_eq!(from_file_name("CTXRemote-Hilfe-482913077-short.exe"), None);
        assert_eq!(from_file_name(&format!("CTXRemote-Hilfe-012345678-{token}.exe")), None, "not a valid ID");
    }
}
