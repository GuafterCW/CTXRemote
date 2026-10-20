//! The team's policy (see `docs/ACCOUNTS.md`, "Richtlinien"): owners and
//! admins decide what every computer opened to the team allows, and those
//! computers enforce it. A policy only takes away; what a computer's own
//! settings forbid stays forbidden.
//!
//! It is encrypted with the report key, which hosts opened to the team hold
//! in their grant, so a service host in no account can fetch it with the
//! report token like it writes reports. Members can read it the same way;
//! only owners and admins may store one, the server checks.

use anyhow::{bail, Result};
use ctxremote_proto::account::{AccountOp, AccountReply, TeamRole};
use ctxremote_proto::session::Permissions;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::account::{open, request, seal};
use crate::report::ReportKey;
use crate::team::{TeamLink, TeamView};

const POLICY_AAD: &[u8] = b"ctxremote/team-policy/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamPolicy {
    /// Rights taken away from technicians in sessions through the team.
    pub deny_technician: Permissions,
    /// The same for owners and admins.
    pub deny_admin: Permissions,
    /// The same for every session not through the team (passwords, the
    /// devices of the computer's own account).
    pub deny_others: Permissions,
    /// Which ways in stay open besides the team.
    pub one_time: bool,
    pub permanent: bool,
    pub account: bool,
    /// The permanent password only together with the authenticator code.
    pub require_code: bool,
    /// Unix seconds of the last change, shown in the app.
    pub changed: u64,
}

impl Default for TeamPolicy {
    fn default() -> Self {
        Self {
            deny_technician: Permissions(0),
            deny_admin: Permissions(0),
            deny_others: Permissions(0),
            one_time: true,
            permanent: true,
            account: true,
            require_code: false,
            changed: 0,
        }
    }
}

impl TeamPolicy {
    /// What is taken away in a session of someone with `role` in the team,
    /// or of someone not through the team (`None`).
    pub fn denied(&self, role: Option<TeamRole>) -> Permissions {
        match role {
            None => self.deny_others,
            Some(role) if role.manages() => self.deny_admin,
            Some(_) => self.deny_technician,
        }
    }
}

/// `rights` without what `deny` takes away.
pub fn capped(rights: Permissions, deny: Permissions) -> Permissions {
    Permissions(rights.0 & !deny.0)
}

/// The policy a host keeps from its last fetch, for the team it is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldPolicy {
    /// Team (hex).
    pub team: String,
    pub policy: TeamPolicy,
}

fn aad(team: &[u8; 16], epoch: u32) -> Vec<u8> {
    [POLICY_AAD, team.as_slice(), &epoch.to_be_bytes()].concat()
}

fn encrypt(key: &ReportKey, policy: &TeamPolicy) -> Result<Vec<u8>> {
    let (team, report_key, _) = key.parts()?;
    seal(&report_key, &aad(&team, key.epoch), &serde_json::to_vec(policy)?)
}

fn decrypt(report_key: &[u8; 32], team: &[u8; 16], epoch: u32, blob: &[u8]) -> Result<TeamPolicy> {
    Ok(serde_json::from_slice(&open(report_key, &aad(team, epoch), blob)?)?)
}

/// The stored policy and the epoch it is encrypted for; asks with the
/// report token of `key`.
async fn stored(server: &str, signing: &SigningKey, key: &ReportKey) -> Result<(u32, Vec<u8>)> {
    let (team, _, token) = key.parts()?;
    match request(server, signing, AccountOp::GetPolicy { team, epoch: key.epoch, token }).await? {
        AccountReply::Policy { epoch, blob } => Ok((epoch, blob)),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// For a host: the team's policy now (`None`: it has none). An error if the
/// server could not be asked, refused the token (the team moved to a new
/// epoch and this grant is not renewed yet) or the policy is for another
/// epoch; the host then keeps the one it had.
pub async fn fetch(server: &str, signing: &SigningKey, key: &ReportKey) -> Result<Option<TeamPolicy>> {
    let (epoch, blob) = stored(server, signing, key).await?;
    if blob.is_empty() {
        return Ok(None);
    }
    if epoch != key.epoch {
        bail!("Die Richtlinie ist für einen anderen Teamschlüssel");
    }
    let (team, report_key, _) = key.parts()?;
    decrypt(&report_key, &team, epoch, &blob).map(Some)
}

/// For members: the team's policy, also one still encrypted for an
/// earlier epoch (with a key from the book).
pub async fn load(server: &str, signing: &SigningKey, view: &TeamView) -> Result<Option<TeamPolicy>> {
    let (epoch, blob) = stored(server, signing, &ReportKey::for_team(&view.link)?).await?;
    if blob.is_empty() {
        return Ok(None);
    }
    let Some(team_key) = crate::report::team_key_of(view, epoch) else {
        bail!("Für diese Richtlinie fehlt der Teamschlüssel");
    };
    let team: [u8; 16] = hex::decode(&view.link.id).ok().and_then(|b| b.try_into().ok()).unwrap_or_default();
    let (report_key, _) = crate::report::secrets(&team_key);
    decrypt(&report_key, &team, epoch, &blob).map(Some)
}

/// Owners and admins: stores `policy` for the team, `None` removes it.
pub async fn save(server: &str, signing: &SigningKey, link: &TeamLink, policy: Option<&TeamPolicy>) -> Result<()> {
    let key = ReportKey::for_team(link)?;
    let blob = match policy {
        Some(policy) => encrypt(&key, policy)?,
        None => Vec::new(),
    };
    request(server, signing, AccountOp::SetPolicy { epoch: link.epoch, blob }).await.map(|_| ())
}

/// Owners and admins, after a new epoch: encrypts the policy anew, so hosts
/// with the new key can read it. A failure leaves it for the next sync.
pub(crate) async fn carry_over(server: &str, signing: &SigningKey, view: &TeamView) {
    let carry = async {
        let (epoch, blob) = stored(server, signing, &ReportKey::for_team(&view.link)?).await?;
        if blob.is_empty() || epoch == view.link.epoch {
            return Ok(());
        }
        let policy = load(server, signing, view).await?;
        save(server, signing, &view.link, policy.as_ref()).await
    };
    if let Err(e) = carry.await {
        tracing::debug!("Richtlinie noch nicht übertragen: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_by_role_and_only_takes_away() {
        let policy = TeamPolicy {
            deny_technician: Permissions(Permissions::TERMINAL | Permissions::TUNNEL),
            deny_others: Permissions(Permissions::FILES),
            ..TeamPolicy::default()
        };
        assert_eq!(policy.denied(Some(TeamRole::Technician)), policy.deny_technician);
        assert_eq!(policy.denied(Some(TeamRole::Admin)), Permissions(0));
        assert_eq!(policy.denied(Some(TeamRole::Owner)), Permissions(0));
        assert_eq!(policy.denied(None), policy.deny_others);
        let rights = capped(Permissions::ATTENDED, policy.deny_technician);
        assert_eq!(rights, Permissions::ATTENDED);
        let rights = capped(Permissions::ALL, policy.deny_technician);
        assert!(!rights.has(Permissions::TERMINAL) && !rights.has(Permissions::TUNNEL) && rights.has(Permissions::FILES));
    }

    #[test]
    fn missing_fields_allow_everything() {
        let policy: TeamPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(policy, TeamPolicy::default());
        assert!(policy.one_time && policy.permanent && policy.account && !policy.require_code);
    }

    /// The web interface (web/scripts/vectors.mjs) must read and write the same.
    #[test]
    fn sealed_for_the_team_and_epoch() {
        let (report_key, _) = crate::report::secrets(&[7; 32]);
        let policy = TeamPolicy { deny_technician: Permissions(Permissions::TERMINAL), changed: 5, ..TeamPolicy::default() };
        let blob = seal(&report_key, &aad(&[1; 16], 2), &serde_json::to_vec(&policy).unwrap()).unwrap();
        assert_eq!(decrypt(&report_key, &[1; 16], 2, &blob).unwrap(), policy);
        assert!(decrypt(&report_key, &[1; 16], 3, &blob).is_err());
        assert!(decrypt(&report_key, &[2; 16], 2, &blob).is_err());
        assert_eq!(
            String::from_utf8(serde_json::to_vec(&policy).unwrap()).unwrap(),
            r#"{"denyTechnician":128,"denyAdmin":0,"denyOthers":0,"oneTime":true,"permanent":true,"account":true,"requireCode":false,"changed":5}"#
        );
        // Sealed once for the web interface, which opens the same.
        let fixed = hex::decode("a2d543def81f0944867d2d2cc870dc74be8ba4b7d73d5c3e550b7f7368da05d458c0039c87cbd9dc627905936c209673a10dc17743f1fb02a33f57d01207").unwrap();
        assert_eq!(decrypt(&report_key, &[1; 16], 2, &fixed).unwrap(), TeamPolicy { changed: 5, ..policy });
    }
}
