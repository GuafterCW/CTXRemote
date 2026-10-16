//! What the public status page shows: whether the server runs, since when,
//! how many devices are online, and how much of each of the last 90 days it
//! was up.
//!
//! Uptime is counted by the server itself, one tick per minute into
//! `<data>/status.json`; minutes it was down are minutes without a tick. Only
//! counts are kept, nothing about devices or people.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Days the page shows.
pub const DAYS: u64 = 90;
const DAY: u64 = 86_400;

/// Counts per UTC day (days since 1970).
#[derive(Default, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
struct Day {
    /// Minutes the server was running.
    minutes: u32,
    /// Sessions the server introduced.
    sessions: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    /// When counting began (Unix seconds); days before it have no data.
    first: u64,
    days: BTreeMap<u64, Day>,
}

pub struct Status {
    path: Option<PathBuf>,
    stored: std::sync::Mutex<Stored>,
    /// When this process started (Unix seconds).
    pub started: u64,
    /// Sessions being relayed right now.
    pub relaying: AtomicUsize,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Status {
    /// Reads the counts kept so far; a missing or broken file starts afresh.
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut stored: Stored = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let started = now();
        if stored.first == 0 {
            stored.first = started;
        }
        Self { path, stored: std::sync::Mutex::new(stored), started, relaying: AtomicUsize::new(0) }
    }

    /// One more minute up, at `at`.
    pub fn tick(&self, at: u64) {
        let mut stored = self.stored.lock().unwrap();
        stored.days.entry(at / DAY).or_default().minutes += 1;
        let oldest = (at / DAY).saturating_sub(DAYS);
        stored.days.retain(|&day, _| day > oldest);
        self.save(&stored);
    }

    /// One more session introduced (not saved until the next tick).
    pub fn session(&self, at: u64) {
        self.stored.lock().unwrap().days.entry(at / DAY).or_default().sessions += 1;
    }

    fn save(&self, stored: &Stored) {
        let Some(path) = &self.path else { return };
        let Ok(bytes) = serde_json::to_vec(stored) else { return };
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path)).is_err() {
            tracing::warn!("Statusdaten nicht gespeichert");
        }
    }

    /// The last [`DAYS`] days up to `at`, oldest first: each day's start
    /// (Unix seconds), its share of uptime (`None` before counting began) and
    /// its sessions.
    pub fn days(&self, at: u64) -> Vec<(u64, Option<f64>, u32)> {
        let stored = self.stored.lock().unwrap();
        let today = at / DAY;
        (today + 1 - DAYS..=today)
            .map(|day| {
                let start = day * DAY;
                let counted = stored.days.get(&day).copied().unwrap_or_default();
                // The minutes that could have been counted: from the day's
                // start (or when counting began) to its end (or now).
                let from = start.max(stored.first);
                let to = (start + DAY).min(at);
                let possible = to.saturating_sub(from) / 60;
                let share = if to <= from {
                    None
                } else if possible == 0 {
                    Some(1.0)
                } else {
                    Some((counted.minutes as f64 / possible as f64).min(1.0))
                };
                (start, share, counted.sessions)
            })
            .collect()
    }
}

/// The server's version, as the release built it.
pub fn version() -> &'static str {
    option_env!("CTXREMOTE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Counts a relayed session while it lasts.
pub struct Relaying<'a>(&'a AtomicUsize);

impl<'a> Relaying<'a> {
    pub fn start(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for Relaying<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_count_minutes_since_counting_began() {
        let day = 20_000 * DAY;
        let status = Status::open(None);
        status.stored.lock().unwrap().first = day + 12 * 3600;
        // Up for 6 of the 12 hours since counting began.
        for minute in 0..360 {
            status.tick(day + 12 * 3600 + minute * 60);
        }
        status.session(day + 13 * 3600);
        let days = status.days(day + DAY - 1);
        assert_eq!(days.len(), DAYS as usize);
        let (start, share, sessions) = *days.last().unwrap();
        assert_eq!(start, day);
        assert!((share.unwrap() - 0.5).abs() < 0.01, "{share:?}");
        assert_eq!(sessions, 1);
        assert_eq!(days[0].1, None, "before counting began there is no data");
    }

    #[test]
    fn counts_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("ctxremote-status-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.json");
        let at = now();
        Status::open(Some(path.clone())).tick(at);
        let again = Status::open(Some(path));
        assert_eq!(again.stored.lock().unwrap().days.get(&(at / DAY)).map(|d| d.minutes), Some(1));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
