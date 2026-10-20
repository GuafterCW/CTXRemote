//! The connection log of a host: who connected when, for how long and how
//! (one-time password, permanent password, account), plus refused attempts.
//! Kept next to the config (`config.history.json`), newest last, at most
//! [`MAX_VISITS`] entries. Only this computer has it; nothing goes to the server.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const MAX_VISITS: usize = 200;

/// How a viewer got in, or why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    OneTimePassword,
    PermanentPassword,
    /// A device of the account, without a password.
    Account,
    /// The password did not fit.
    WrongPassword,
    /// The account password fitted, but the device is no member (any more).
    NotMember,
    /// The person at this computer said no (or did not answer).
    Declined,
    /// The permanent password fitted, the code from the authenticator app not.
    WrongCode,
    /// A member of the team, without a password.
    Team,
    /// The team's policy does not allow this way in (see `crate::policy`).
    Policy,
}

impl Outcome {
    pub fn admitted(self) -> bool {
        matches!(self, Self::OneTimePassword | Self::PermanentPassword | Self::Account | Self::Team)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Visit {
    /// Unix seconds.
    pub started: u64,
    /// Unix seconds; `None` while the session runs (or if the computer went
    /// off during it).
    pub ended: Option<u64>,
    /// How the viewer named itself ("user (PC)"); empty when it failed
    /// before saying so, e.g. with a wrong password.
    pub peer: String,
    /// The name of its profile, if it sent one (self-declared).
    #[serde(default)]
    pub profile: Option<String>,
    pub outcome: Outcome,
    /// What the viewer did beyond watching and controlling: a command line
    /// opened, processes ended, services started or stopped. At most
    /// [`MAX_ACTIONS`]; the last one says when more were left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

/// Actions kept per visit; a session ending hundreds of processes would
/// otherwise grow the log without bound.
pub const MAX_ACTIONS: usize = 50;

pub struct History {
    path: Option<PathBuf>,
    visits: Mutex<Vec<Visit>>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl History {
    /// Loads the log at `path`; without a path it lives in memory only.
    pub fn open(path: Option<PathBuf>) -> Self {
        let visits = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self { path, visits: Mutex::new(visits) }
    }

    /// The log file belonging to the config file at `config`.
    pub fn path_for(config: &std::path::Path) -> PathBuf {
        config.with_extension("history.json")
    }

    /// Newest first.
    pub fn visits(&self) -> Vec<Visit> {
        let mut visits = self.visits.lock().unwrap().clone();
        visits.reverse();
        visits
    }

    /// Adds an entry; returns its start time for [`History::end`].
    pub fn add(&self, peer: &str, profile: Option<&str>, outcome: Outcome) -> u64 {
        let started = now();
        let mut visits = self.visits.lock().unwrap();
        let ended = (!outcome.admitted()).then_some(started);
        visits.push(Visit {
            started,
            ended,
            peer: peer.to_string(),
            profile: profile.map(str::to_string),
            outcome,
            actions: Vec::new(),
        });
        let excess = visits.len().saturating_sub(MAX_VISITS);
        visits.drain(..excess);
        self.save(&visits);
        started
    }

    /// Marks the session that started at `started` with `peer` as ended.
    pub fn end(&self, started: u64, peer: &str) {
        let mut visits = self.visits.lock().unwrap();
        if let Some(visit) = visits.iter_mut().rev().find(|v| v.started == started && v.peer == peer && v.ended.is_none()) {
            visit.ended = Some(now().max(started));
            self.save(&visits);
        }
    }

    /// Notes what the viewer of the session that started at `started` with
    /// `peer` did.
    pub fn act(&self, started: u64, peer: &str, action: &str) {
        let mut visits = self.visits.lock().unwrap();
        let Some(visit) = visits.iter_mut().rev().find(|v| v.started == started && v.peer == peer) else { return };
        match visit.actions.len() {
            n if n + 1 < MAX_ACTIONS => visit.actions.push(action.to_string()),
            n if n + 1 == MAX_ACTIONS => visit.actions.push("… weitere Aktionen nicht aufgeführt".to_string()),
            _ => return,
        }
        self.save(&visits);
    }

    /// What was noted with [`History::act`] for that session.
    pub fn actions(&self, started: u64, peer: &str) -> Vec<String> {
        let visits = self.visits.lock().unwrap();
        visits.iter().rev().find(|v| v.started == started && v.peer == peer).map(|v| v.actions.clone()).unwrap_or_default()
    }

    fn save(&self, visits: &[Visit]) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec(visits)?)?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            tracing::warn!("Verbindungsprotokoll nicht gespeichert: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_actions_up_to_a_limit() {
        let history = History::open(None);
        let started = history.add("anna (LAPTOP)", None, Outcome::Team);
        history.act(started, "anna (LAPTOP)", "Terminal geöffnet");
        history.act(started, "someone else", "Prozess x beendet");
        assert_eq!(history.actions(started, "anna (LAPTOP)"), ["Terminal geöffnet"]);
        for n in 0..100 {
            history.act(started, "anna (LAPTOP)", &format!("Prozess {n} beendet"));
        }
        let actions = history.actions(started, "anna (LAPTOP)");
        assert_eq!(actions.len(), MAX_ACTIONS);
        assert!(actions.last().unwrap().starts_with('…'));
        // Old logs without actions still load, and empty ones are not written.
        let json = serde_json::to_string(&history.visits()[0]).unwrap();
        let mut visit: Visit = serde_json::from_str(&json).unwrap();
        visit.actions.clear();
        assert!(!serde_json::to_string(&visit).unwrap().contains("actions"));
    }

    #[test]
    fn keeps_the_newest_and_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("ctxremote-history-{}.json", rand::random::<u32>()));
        let history = History::open(Some(path.clone()));
        history.add("", None, Outcome::WrongPassword);
        let started = history.add("anna (LAPTOP)", Some("Anna"), Outcome::Account);
        assert_eq!(history.visits()[0].ended, None);
        assert!(history.visits()[1].ended.is_some(), "refusals end at once");
        history.end(started, "anna (LAPTOP)");

        let again = History::open(Some(path.clone()));
        let visits = again.visits();
        assert_eq!(visits.len(), 2);
        assert_eq!((visits[0].outcome, visits[0].profile.as_deref()), (Outcome::Account, Some("Anna")));
        assert!(visits[0].ended.is_some());

        for _ in 0..MAX_VISITS + 5 {
            again.add("x", None, Outcome::OneTimePassword);
        }
        assert_eq!(again.visits().len(), MAX_VISITS);
        let _ = std::fs::remove_file(path);
    }
}
