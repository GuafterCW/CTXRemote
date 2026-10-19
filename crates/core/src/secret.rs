//! Secrets in the config file (the permanent password, the authenticator
//! secret), encrypted with DPAPI on Windows.
//!
//! The service's `host.json` is bound to the machine (any process on it may
//! decrypt, but the file is readable only by SYSTEM and administrators), so
//! the installer, running as an administrator, and the service, running as
//! SYSTEM, both read what the other wrote. A user's own config is bound to
//! that user's Windows account. Either way a copy of the file, say from a
//! backup or a disk image, is useless on another computer.
//!
//! Elsewhere, and in files from older versions, the value stays plain text.

use std::sync::atomic::{AtomicBool, Ordering};

/// Marks a DPAPI blob (base64) in the file.
const PREFIX: &str = "dpapi:";

static MACHINE: AtomicBool = AtomicBool::new(false);

/// Binds secrets written from now on to the machine instead of the user.
pub fn bind_to_machine() {
    MACHINE.store(true, Ordering::Relaxed);
}

/// The form stored in the file.
pub fn protect(plain: &str) -> String {
    #[cfg(windows)]
    match dpapi::protect(plain.as_bytes(), MACHINE.load(Ordering::Relaxed)) {
        Ok(blob) => {
            use base64::Engine;
            return format!("{PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(blob));
        }
        Err(e) => tracing::warn!("Geheimnis nicht verschlüsselbar, bleibt Klartext: {e}"),
    }
    plain.to_string()
}

/// Reads the stored form back; `None` if it cannot be decrypted here (a
/// file from another computer or account), which drops the secret.
pub fn unprotect(stored: &str) -> Option<String> {
    let Some(blob) = stored.strip_prefix(PREFIX) else {
        return Some(stored.to_string());
    };
    let result = decrypt(blob);
    if let Err(e) = &result {
        tracing::warn!("Gespeichertes Geheimnis nicht lesbar, wird verworfen: {e}");
    }
    result.ok()
}

#[cfg(windows)]
fn decrypt(blob: &str) -> anyhow::Result<String> {
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD.decode(blob)?;
    Ok(String::from_utf8(dpapi::unprotect(&blob)?)?)
}

#[cfg(not(windows))]
fn decrypt(_blob: &str) -> anyhow::Result<String> {
    anyhow::bail!("DPAPI gibt es nur unter Windows")
}

/// Whether any of these top-level string fields of the JSON is stored as
/// plain text.
pub fn has_plain(json: &str, fields: &[&str]) -> bool {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json) else { return false };
    fields.iter().any(|f| map.get(*f).and_then(|v| v.as_str()).is_some_and(|v| !v.starts_with(PREFIX)))
}

/// For `#[serde(with = "crate::secret::opt")]` on `Option<String>` fields.
pub mod opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(plain) => serializer.serialize_some(&super::protect(plain)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.and_then(|stored| super::unprotect(&stored)))
    }
}

#[cfg(windows)]
mod dpapi {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
        CRYPT_INTEGER_BLOB,
    };

    /// Ties blobs to this program: other DPAPI users on the machine cannot
    /// decrypt them by accident.
    const ENTROPY: &[u8] = b"ctxremote/config-secret/v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Copies the system-allocated output and frees it.
    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: on success DPAPI hands over `cbData` bytes at `pbData`,
        // allocated with LocalAlloc.
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(Some(HLOCAL(out.pbData.cast())));
            bytes
        }
    }

    pub fn protect(data: &[u8], machine: bool) -> windows::core::Result<Vec<u8>> {
        let flags = CRYPTPROTECT_UI_FORBIDDEN | if machine { CRYPTPROTECT_LOCAL_MACHINE } else { 0 };
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: input and entropy outlive the call; `out` is freed in `take`.
        unsafe { CryptProtectData(&input, PCWSTR::null(), Some(&entropy), None, None, flags, &mut out)? };
        Ok(take(out))
    }

    pub fn unprotect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`. The blob itself records user or machine scope.
        unsafe {
            CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?
        };
        Ok(take(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_values_from_older_files_still_load() {
        assert_eq!(unprotect("hunter2").as_deref(), Some("hunter2"));
    }

    #[test]
    fn stored_values_load_back() {
        let stored = protect("Geheim 123 äö");
        assert_eq!(cfg!(windows), stored.starts_with(PREFIX));
        assert_eq!(unprotect(&stored).as_deref(), Some("Geheim 123 äö"));
    }

    #[test]
    fn finds_plain_fields() {
        let fields = ["permanent_password", "code_secret"];
        assert!(has_plain(r#"{"permanent_password":"x","code_secret":null}"#, &fields));
        assert!(!has_plain(r#"{"permanent_password":"dpapi:AAAA","code_secret":null}"#, &fields));
        assert!(!has_plain(r#"{"server":"a"}"#, &fields));
    }

    #[test]
    fn unreadable_blobs_are_dropped() {
        assert_eq!(unprotect("dpapi:AAAA"), None);
        assert_eq!(unprotect("dpapi:kein base64"), None);
    }
}
