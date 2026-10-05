//! Codes from an authenticator app (TOTP, RFC 6238): HMAC-SHA1, 30-second
//! steps, six digits, which is what common apps expect by default.
//!
//! The permanent password is the first factor; with a secret set, the host
//! also asks for the current code (see `host::serve`).

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;

const STEP: u64 = 30;
const DIGITS: u32 = 6;
/// Steps before and after the current one that are still accepted, for clocks
/// that are a little off.
const SKEW: u64 = 1;
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A new secret, base32 as authenticator apps take it (160 bits).
pub fn new_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base32(&bytes)
}

/// The link an authenticator app reads from the QR code.
pub fn uri(secret: &str, account: &str) -> String {
    let label: String = account
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("otpauth://totp/CTXRemote:{label}?secret={secret}&issuer=CTXRemote&digits={DIGITS}&period={STEP}")
}

/// The step `code` matches, if it is valid for `secret` at `unix` seconds.
/// Callers remember the step to refuse the same code a second time.
pub fn check(secret: &str, code: &str, unix: u64) -> Option<u64> {
    let key = unbase32(secret)?;
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let now = unix / STEP;
    (now.saturating_sub(SKEW)..=now + SKEW).find(|&step| format_code(hotp(&key, step)) == code)
}

/// The code for `secret` at `unix` seconds, as the app would show it.
pub fn code_at(secret: &str, unix: u64) -> Option<String> {
    Some(format_code(hotp(&unbase32(secret)?, unix / STEP)))
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn hotp(key: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[19] & 0x0f) as usize;
    let value = u32::from_be_bytes([hash[offset] & 0x7f, hash[offset + 1], hash[offset + 2], hash[offset + 3]]);
    value % 10u32.pow(DIGITS)
}

fn format_code(value: u32) -> String {
    format!("{value:0width$}", width = DIGITS as usize)
}

fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let value = ALPHABET.iter().position(|&a| a as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238, appendix B, SHA-1 with the key "12345678901234567890".
    #[test]
    fn rfc_vectors() {
        let secret = base32(b"12345678901234567890");
        assert_eq!(secret, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        for (time, code) in [(59, "287082"), (1111111109, "081804"), (1234567890, "005924"), (2000000000, "279037")] {
            // The vectors have eight digits; six are their last six.
            let key = unbase32(&secret).unwrap();
            assert_eq!(format_code(hotp(&key, time / STEP)), code, "Zeit {time}");
            assert_eq!(check(&secret, code, time), Some(time / STEP));
        }
    }

    #[test]
    fn window_and_format() {
        let secret = new_secret();
        assert_eq!(secret.len(), 32);
        let key = unbase32(&secret).unwrap();
        let t = 1_700_000_000;
        let code = format_code(hotp(&key, t / STEP - 1));
        assert!(check(&secret, &code, t).is_some(), "ein Schritt alt geht noch");
        assert!(check(&secret, &code, t + 3 * STEP).is_none(), "drei Schritte zu alt");
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        assert!(check(&secret, &spaced, t).is_some(), "Leerzeichen stören nicht");
        assert!(check(&secret, "12345", t).is_none());
        assert!(check("kein base32!", &code, t).is_none());
        assert!(uri(&secret, "Büro PC").starts_with("otpauth://totp/CTXRemote:B%C3%BCro%20PC?secret="));
    }
}
