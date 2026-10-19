//! Session reports for a team (see `docs/ACCOUNTS.md`, "Sitzungsberichte"):
//! members' apps report the sessions they open, hosts opened to the team
//! the sessions they take. Each report is encrypted with a key derived from
//! the team key; the server stores it and gives it only to owners and
//! admins. Writing needs the report token, also derived from the team key,
//! whose hash an admin's app hands the server.
//!
//! Reports wait in a small queue file until the server took them, and are
//! encrypted only when sent, with the key of the epoch at that time.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use ctxremote_proto::account::{AccountError, AccountOp, AccountReply, ReportEntry, TeamMember};
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::account::{open, request, seal};
use crate::history::Outcome;
use crate::team::{TeamLink, TeamView};

const REPORT_AAD: &[u8] = b"ctxremote/team-report/v1";
/// Reports kept while the server cannot be reached.
const MAX_QUEUED: usize = 200;

/// Which end of the session reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    /// A member connected to another computer.
    Viewer,
    /// Someone connected to a computer opened to the team.
    Host,
}

/// One session, as reported; encrypted on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub side: Side,
    /// The reporting device: its ID and name.
    pub device: Option<u32>,
    pub device_name: String,
    /// The other end: its ID if known (the viewer knows it), and its name.
    #[serde(default)]
    pub peer: Option<u32>,
    #[serde(default)]
    pub peer_name: String,
    /// The viewer's profile name, if it sent one (host side, self-declared).
    #[serde(default)]
    pub profile: Option<String>,
    /// Unix seconds.
    pub started: u64,
    pub ended: u64,
    /// How the viewer got in, or why not (host side).
    #[serde(default)]
    pub outcome: Option<Outcome>,
}

/// What a device needs to write reports: kept with a team grant on hosts,
/// derived from the team key in the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportKey {
    /// Team (hex) and epoch.
    pub team: String,
    pub epoch: u32,
    /// Hex.
    pub key: String,
    pub token: String,
}

/// The report key and token of a team key.
pub fn secrets(team_key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let hk = Hkdf::<Sha256>::new(Some(REPORT_AAD), team_key);
    let (mut key, mut token) = ([0u8; 32], [0u8; 32]);
    hk.expand(b"key", &mut key).expect("32 bytes");
    hk.expand(b"token", &mut token).expect("32 bytes");
    (key, token)
}

/// What the server keeps to check the token.
pub fn check(token: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(token).into()
}

impl ReportKey {
    pub fn for_team(link: &TeamLink) -> Result<Self> {
        let (key, token) = secrets(&link.key()?);
        Ok(Self { team: link.id.clone(), epoch: link.epoch, key: hex::encode(key), token: hex::encode(token) })
    }

    fn parts(&self) -> Result<([u8; 16], [u8; 32], [u8; 32])> {
        let team = hex::decode(&self.team).ok().and_then(|b| b.try_into().ok());
        let key = hex::decode(&self.key).ok().and_then(|b| b.try_into().ok());
        let token = hex::decode(&self.token).ok().and_then(|b| b.try_into().ok());
        match (team, key, token) {
            (Some(t), Some(k), Some(x)) => Ok((t, k, x)),
            _ => bail!("Berichtsschlüssel ist beschädigt"),
        }
    }
}

fn aad(team: &[u8; 16], epoch: u32) -> Vec<u8> {
    [REPORT_AAD, team.as_slice(), &epoch.to_be_bytes()].concat()
}

pub fn encrypt(key: &ReportKey, report: &Report) -> Result<Vec<u8>> {
    let (team, report_key, _) = key.parts()?;
    seal(&report_key, &aad(&team, key.epoch), &serde_json::to_vec(report)?)
}

/// Owners and admins: hands the server the check for this epoch's token.
pub async fn set_check(server: &str, signing: &SigningKey, link: &TeamLink) -> Result<()> {
    let (_, token) = secrets(&link.key()?);
    request(server, signing, AccountOp::SetReportCheck { epoch: link.epoch, check: check(&token) }).await.map(|_| ())
}

/// Reports waiting to be sent, kept in a file next to the config.
pub struct Outbox {
    path: Option<PathBuf>,
    queue: Mutex<Vec<Report>>,
    /// One sender at a time.
    turn: tokio::sync::Mutex<()>,
}

impl Outbox {
    pub fn open(path: Option<PathBuf>) -> Self {
        let queue = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self { path, queue: Mutex::new(queue), turn: tokio::sync::Mutex::new(()) }
    }

    /// The queue file belonging to the config file at `config`; `side`
    /// keeps the app's and the host's apart where they share a folder.
    pub fn path_for(config: &Path, side: Side) -> PathBuf {
        config.with_extension(match side {
            Side::Viewer => "reports-viewer.json",
            Side::Host => "reports-host.json",
        })
    }

    pub fn push(&self, report: Report) {
        let mut queue = self.queue.lock().unwrap();
        queue.push(report);
        let excess = queue.len().saturating_sub(MAX_QUEUED);
        queue.drain(..excess);
        self.save(&queue);
    }

    pub fn len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sends what is queued, oldest first; stops at the first failure and
    /// keeps the rest. A report the server refuses for good is dropped.
    pub async fn flush(&self, server: &str, signing: &SigningKey, key: &ReportKey) -> Result<()> {
        let _turn = self.turn.lock().await;
        let (team, _, token) = key.parts()?;
        loop {
            let Some(report) = self.queue.lock().unwrap().first().cloned() else { return Ok(()) };
            let blob = encrypt(key, &report)?;
            let op = AccountOp::AddReport { team, epoch: key.epoch, token, blob };
            match request(server, signing, op).await {
                Ok(_) => {}
                Err(e) if e.downcast_ref::<AccountError>() == Some(&AccountError::TooLarge) => {
                    tracing::warn!("Sitzungsbericht zu groß, verworfen");
                }
                Err(e) => return Err(e),
            }
            let mut queue = self.queue.lock().unwrap();
            if queue.first() == Some(&report) {
                queue.remove(0);
            }
            self.save(&queue);
        }
    }

    fn save(&self, queue: &[Report]) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec(queue)?)?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            tracing::warn!("Sitzungsberichte nicht gespeichert: {e}");
        }
    }
}

/// A stored report, decrypted where possible.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRow {
    pub seq: u64,
    /// When the server got it (Unix seconds).
    pub at: u64,
    /// The member whose device sent it, as the server knows it.
    pub member: Option<u64>,
    pub member_email: Option<String>,
    /// `None`: no key for its epoch, or damaged.
    pub report: Option<Report>,
}

/// The key of `epoch`: the current one or one of the book's past keys.
fn team_key_of(view: &TeamView, epoch: u32) -> Option<[u8; 32]> {
    if epoch == view.link.epoch {
        return view.link.key().ok();
    }
    hex::decode(view.book.past.get(&epoch)?).ok()?.try_into().ok()
}

pub fn decrypt(view: &TeamView, entry: &ReportEntry) -> Option<Report> {
    let team: [u8; 16] = hex::decode(&view.link.id).ok()?.try_into().ok()?;
    let (key, _) = secrets(&team_key_of(view, entry.epoch)?);
    let plain = open(&key, &aad(&team, entry.epoch), &entry.blob).ok()?;
    serde_json::from_slice(&plain).ok()
}

fn email(members: &[TeamMember], member: Option<u64>) -> Option<String> {
    members.iter().find(|m| Some(m.id) == member)?.email.clone()
}

/// All stored reports of the team (owners, admins), oldest first.
pub async fn fetch(server: &str, signing: &SigningKey, view: &TeamView) -> Result<Vec<ReportRow>> {
    let mut rows = Vec::new();
    let mut after = 0;
    loop {
        let (entries, more) = match request(server, signing, AccountOp::GetReports { after }).await? {
            AccountReply::Reports { entries, more } => (entries, more),
            other => bail!("unerwartete Serverantwort: {other:?}"),
        };
        for entry in &entries {
            rows.push(ReportRow {
                seq: entry.seq,
                at: entry.at,
                member: entry.member,
                member_email: email(&view.members, entry.member),
                report: decrypt(view, entry),
            });
        }
        let last = entries.last().map(|e| e.seq).context("leere Antwort")?;
        if !more {
            return Ok(rows);
        }
        after = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_stay_stable() {
        // The web interface derives the same; web/vectors.mjs checks it.
        let (key, token) = secrets(&[7; 32]);
        assert_eq!(hex::encode(key), "a1ad62d9a59100295c4e606058f0316993c00e849a46c025cab485a08da893f9");
        assert_eq!(hex::encode(token), "67ad34d1b923db354266bf4f7c4d060528cfe73f1af8d589955a9f8c24b6da9f");
        assert_eq!(hex::encode(check(&token)), "fb8bca47970036236846afcb74efee14cb05a7a166339c4695e0b8be3844b1c8");
    }

    #[test]
    fn the_queue_survives_a_restart_and_stays_small() {
        let path = std::env::temp_dir().join(format!("ctxremote-reports-{}.json", rand::random::<u32>()));
        let outbox = Outbox::open(Some(path.clone()));
        let report = Report {
            side: Side::Viewer,
            device: Some(1),
            device_name: "A".into(),
            peer: Some(2),
            peer_name: "B".into(),
            profile: None,
            started: 10,
            ended: 20,
            outcome: None,
        };
        for _ in 0..MAX_QUEUED + 3 {
            outbox.push(report.clone());
        }
        assert_eq!(Outbox::open(Some(path.clone())).len(), MAX_QUEUED);
        let _ = std::fs::remove_file(path);
    }
}
