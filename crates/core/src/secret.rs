//! Secrets in the config file (the permanent password, the authenticator
//! secret): encrypted with DPAPI on Windows, in the desktop's keyring
//! (Secret Service) on Linux.
//!
//! The service's `host.json` is bound to the machine (any process on it may
//! decrypt, but the file is readable only by SYSTEM and administrators), so
//! the installer, running as an administrator, and the service, running as
//! SYSTEM, both read what the other wrote. A user's own config is bound to
//! that user's Windows account. Either way a copy of the file, say from a
//! backup or a disk image, is useless on another computer.
//!
//! On Linux the file holds only a reference. Without a keyring (no desktop
//! session, none installed) the value stays in the file, which only its
//! owner can read. A keyring that is locked when the config is read (say
//! after an automatic sign-in) leaves the secret out for this run, but the
//! reference stays in the file, so the next start finds it again.
//!
//! On the Mac, and in files from older versions, the value stays plain text
//! (an unsigned app would have to ask for the keychain after every update).

use std::sync::atomic::{AtomicBool, Ordering};

/// Marks a DPAPI blob (base64) in the file.
const PREFIX: &str = "dpapi:";

static MACHINE: AtomicBool = AtomicBool::new(false);

/// Binds secrets written from now on to the machine instead of the user.
pub fn bind_to_machine() {
    MACHINE.store(true, Ordering::Relaxed);
}

/// The form of `value` stored in the file for `field`.
pub fn protect(field: &str, value: Option<&str>) -> Option<String> {
    #[cfg(target_os = "linux")]
    return linux::protect(field, value);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = field;
        value.map(protect_plain)
    }
}

#[cfg_attr(target_os = "linux", allow(dead_code))] // Linux keeps plain values as they are
fn protect_plain(plain: &str) -> String {
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
pub fn unprotect(field: &str, stored: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    if stored == linux::MARK {
        return linux::load(field);
    }
    let _ = field;
    let Some(blob) = stored.strip_prefix(PREFIX) else {
        return Some(stored.to_string());
    };
    let result = decrypt(blob);
    if let Err(e) = &result {
        tracing::warn!("Gespeichertes Geheimnis nicht lesbar, wird verworfen: {e}");
    }
    result.ok()
}

/// Call when the user changes `field`: a reference to a locked keyring is
/// no longer kept for it.
pub fn forget(field: &str) {
    #[cfg(target_os = "linux")]
    linux::forget(field);
    let _ = field;
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

/// Whether the file still holds some of these top-level string fields as
/// plain text that could be stored safely now.
pub fn has_plain(json: &str, fields: &[&str]) -> bool {
    if !(cfg!(windows) || keyring_available()) {
        return false;
    }
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json) else { return false };
    fields.iter().any(|f| map.get(*f).and_then(|v| v.as_str()).is_some_and(|v| !is_protected(v)))
}

fn keyring_available() -> bool {
    #[cfg(target_os = "linux")]
    return linux::available();
    #[cfg(not(target_os = "linux"))]
    false
}

fn is_protected(stored: &str) -> bool {
    #[cfg(target_os = "linux")]
    if stored == linux::MARK {
        return true;
    }
    stored.starts_with(PREFIX)
}

/// `#[serde(with = "crate::secret::<field>")]` for an `Option<String>` field.
macro_rules! secret_field {
    ($field:ident) => {
        pub mod $field {
            use serde::{Deserialize, Deserializer, Serialize, Serializer};

            pub fn serialize<S: Serializer>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error> {
                super::protect(stringify!($field), value.as_deref()).serialize(serializer)
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
                Ok(Option::<String>::deserialize(deserializer)?
                    .and_then(|stored| super::unprotect(stringify!($field), &stored)))
            }
        }
    };
}

secret_field!(permanent_password);
secret_field!(code_secret);

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use anyhow::{bail, Result};
    use dbus_secret_service::{EncryptionType, SecretService};

    /// Stands in the file for a value kept in the keyring.
    pub const MARK: &str = "keyring:";

    /// What the keyring holds, per entry, as far as this process knows.
    static STORED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    /// Entries the file refers to that could not be read (locked keyring).
    static UNREADABLE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    /// Only in a desktop session: without one, libdbus would try to start a
    /// bus of its own.
    pub fn available() -> bool {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    /// One entry per field and config file (a second profile has its own).
    fn entry(field: &str) -> String {
        let path = crate::config::Config::path().map(|p| p.display().to_string()).unwrap_or_default();
        format!("{field}:{path}")
    }

    fn label(field: &str) -> &'static str {
        match field {
            "permanent_password" => "Remocto: festes Passwort",
            _ => "Remocto: Authenticator-Geheimnis",
        }
    }

    /// Never shows a prompt: a locked keyring is an error, not a dialog
    /// that blocks the save until someone answers it.
    fn connect() -> Result<SecretService> {
        if !available() {
            bail!("keine Desktop-Sitzung");
        }
        Ok(SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0)?)
    }

    fn attributes(entry: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "remocto"), ("entry", entry)])
    }

    fn store(entry: &str, label: &str, value: &str) -> Result<()> {
        let service = connect()?;
        let collection = service.get_default_collection()?;
        if collection.is_locked()? {
            bail!("Schlüsselbund gesperrt");
        }
        collection.create_item(label, attributes(entry), value.as_bytes(), true, "text/plain")?;
        Ok(())
    }

    fn read(entry: &str) -> Result<String> {
        let service = connect()?;
        let found = service.search_items(attributes(entry))?;
        let Some(item) = found.unlocked.first() else {
            bail!(if found.locked.is_empty() { "kein Eintrag" } else { "Schlüsselbund gesperrt" });
        };
        Ok(String::from_utf8(item.get_secret()?)?)
    }

    fn delete(entry: &str) -> Result<()> {
        let service = connect()?;
        for item in service.search_items(attributes(entry))?.unlocked {
            item.delete()?;
        }
        Ok(())
    }

    pub fn protect(field: &str, value: Option<&str>) -> Option<String> {
        let entry = entry(field);
        let mut stored = STORED.lock().unwrap();
        let stored = stored.get_or_insert_with(HashMap::new);
        let mut unreadable = UNREADABLE.lock().unwrap();
        let unreadable = unreadable.get_or_insert_with(HashSet::new);
        let Some(value) = value else {
            if unreadable.contains(&entry) {
                return Some(MARK.to_string());
            }
            if stored.remove(&entry).is_some() {
                if let Err(e) = delete(&entry) {
                    tracing::warn!("Eintrag im Schlüsselbund nicht gelöscht: {e}");
                }
            }
            return None;
        };
        if stored.get(&entry).is_some_and(|known| known == value) {
            return Some(MARK.to_string());
        }
        match store(&entry, label(field), value) {
            Ok(()) => {
                stored.insert(entry.clone(), value.to_string());
                unreadable.remove(&entry);
                Some(MARK.to_string())
            }
            Err(e) => {
                tracing::debug!("Schlüsselbund nicht nutzbar, Geheimnis bleibt in der Datei: {e}");
                stored.remove(&entry);
                unreadable.remove(&entry);
                Some(value.to_string())
            }
        }
    }

    pub fn load(field: &str) -> Option<String> {
        let entry = entry(field);
        match read(&entry) {
            Ok(value) => {
                STORED.lock().unwrap().get_or_insert_with(HashMap::new).insert(entry.clone(), value.clone());
                UNREADABLE.lock().unwrap().get_or_insert_with(HashSet::new).remove(&entry);
                Some(value)
            }
            Err(e) => {
                tracing::warn!("{} nicht lesbar ({e}); bleibt für diesen Start aus", label(field));
                UNREADABLE.lock().unwrap().get_or_insert_with(HashSet::new).insert(entry);
                None
            }
        }
    }

    pub fn forget(field: &str) {
        if let Some(unreadable) = UNREADABLE.lock().unwrap().as_mut() {
            unreadable.remove(&entry(field));
        }
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
        assert_eq!(unprotect("permanent_password", "hunter2").as_deref(), Some("hunter2"));
    }

    #[test]
    fn stored_values_load_back() {
        let stored = protect_plain("Geheim 123 äö");
        assert_eq!(cfg!(windows), stored.starts_with(PREFIX));
        assert_eq!(unprotect("permanent_password", &stored).as_deref(), Some("Geheim 123 äö"));
    }

    #[test]
    fn finds_plain_fields() {
        if !(cfg!(windows) || keyring_available()) {
            return;
        }
        let fields = ["permanent_password", "code_secret"];
        assert!(has_plain(r#"{"permanent_password":"x","code_secret":null}"#, &fields));
        assert!(!has_plain(r#"{"permanent_password":"dpapi:AAAA","code_secret":null}"#, &fields));
        assert!(!has_plain(r#"{"server":"a"}"#, &fields));
    }

    /// Needs a running Secret Service, e.g. `dbus-run-session` with an
    /// unlocked `gnome-keyring-daemon`; passes trivially without one unless
    /// `CTXREMOTE_TEST_KEYRING` is set.
    #[test]
    #[cfg(target_os = "linux")]
    fn keyring_keeps_the_secret_and_a_locked_reference() {
        let field = "test_secret";
        if protect(field, Some("abc")).as_deref() != Some(linux::MARK) {
            assert!(std::env::var_os("CTXREMOTE_TEST_KEYRING").is_none(), "Schlüsselbund nicht nutzbar");
            return;
        }
        assert_eq!(unprotect(field, linux::MARK).as_deref(), Some("abc"));
        // Removed: gone from the keyring too.
        assert_eq!(protect(field, None), None);
        // A reference that cannot be read stays in the file...
        assert_eq!(unprotect(field, linux::MARK), None);
        assert_eq!(protect(field, None).as_deref(), Some(linux::MARK));
        // ...until the user changes the value.
        forget(field);
        assert_eq!(protect(field, None), None);
    }

    #[test]
    fn unreadable_blobs_are_dropped() {
        assert_eq!(unprotect("permanent_password", "dpapi:AAAA"), None);
        assert_eq!(unprotect("permanent_password", "dpapi:kein base64"), None);
    }
}
