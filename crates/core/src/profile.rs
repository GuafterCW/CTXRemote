//! The helper profile as the settings form and the host's windows use it
//! (see [`HelperProfile`] for the wire form and its limits).

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ctxremote_proto::session::HelperProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub company: String,
    pub message: String,
    /// PNG as base64; empty for none.
    pub logo: String,
}

impl Profile {
    /// What goes into the `Hello` trailer; `None` for an empty profile.
    pub fn to_wire(&self) -> Option<HelperProfile> {
        HelperProfile {
            name: self.name.clone(),
            company: self.company.clone(),
            message: self.message.clone(),
            logo: STANDARD.decode(&self.logo).unwrap_or_default(),
        }
        .sanitized()
    }

    /// A received profile, cleaned up; `None` if nothing usable is left.
    pub fn from_wire(profile: HelperProfile) -> Option<Self> {
        let profile = profile.sanitized()?;
        Some(Self {
            name: profile.name,
            company: profile.company,
            message: profile.message,
            logo: if profile.logo.is_empty() { String::new() } else { STANDARD.encode(&profile.logo) },
        })
    }

    /// Checks a profile from the settings form. `Ok(None)` removes it.
    pub fn validate(self) -> Result<Option<Self>, String> {
        if !self.logo.is_empty() {
            let logo = STANDARD.decode(&self.logo).map_err(|_| "Das Logo ist beschädigt".to_string())?;
            if logo.len() > HelperProfile::MAX_LOGO {
                return Err("Das Logo ist zu groß (höchstens 64 KB)".into());
            }
            if !logo.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err("Das Logo muss ein PNG-Bild sein".into());
            }
        }
        if self.name.trim().is_empty() && self.company.trim().is_empty() {
            if self.message.trim().is_empty() && self.logo.is_empty() {
                return Ok(None);
            }
            return Err("Bitte einen Namen oder eine Firma angeben".into());
        }
        Ok(self.to_wire().and_then(Self::from_wire))
    }

    /// One line for lists and logs: "Name (Firma)", or whichever is set.
    pub fn label(&self) -> String {
        match (self.name.is_empty(), self.company.is_empty()) {
            (false, false) => format!("{} ({})", self.name, self.company),
            (false, true) => self.name.clone(),
            _ => self.company.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert_eq!(Profile::default().validate(), Ok(None));
        assert!(Profile { message: "Hallo".into(), ..Default::default() }.validate().is_err());
        assert!(Profile { name: "A".into(), logo: STANDARD.encode(b"GIF89a"), ..Default::default() }.validate().is_err());
        assert!(Profile { name: "A".into(), logo: "%%%".into(), ..Default::default() }.validate().is_err());

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0; 32]);
        let ok = Profile { name: " Philipp ".into(), company: "Ecker IT".into(), logo: STANDARD.encode(&png), ..Default::default() };
        let ok = ok.validate().unwrap().unwrap();
        assert_eq!(ok.name, "Philipp");
        assert_eq!(ok.label(), "Philipp (Ecker IT)");
        // Round trip over the wire.
        assert_eq!(Profile::from_wire(ok.to_wire().unwrap()), Some(ok));
    }
}
