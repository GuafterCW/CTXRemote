//! Automatic bitrate control: on a congested link the capture thread blocks
//! while handing over frames, so the share of time spent blocked says how
//! far the encoder's bitrate exceeds what the connection carries.

use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(1);
/// Blocked for more than this share of a window: the link is full.
const CONGESTED: f32 = 0.25;
/// Blocked for less than this share: the link has room.
const CALM: f32 = 0.05;
/// Calm windows in a row before the bitrate rises again.
const CALM_WINDOWS: u32 = 3;
const MIN_FACTOR: f32 = 0.2;

/// Scales the quality preset's bitrate between [`MIN_FACTOR`] and 1.
pub struct Congestion {
    factor: f32,
    window_start: Instant,
    blocked: Duration,
    calm: u32,
}

impl Congestion {
    pub fn new(now: Instant) -> Self {
        Self { factor: 1.0, window_start: now, blocked: Duration::ZERO, calm: 0 }
    }

    pub fn factor(&self) -> f32 {
        self.factor
    }

    /// Records how long handing over one frame blocked. At the end of a
    /// window, returns the new factor if it changed.
    pub fn record(&mut self, blocked: Duration, now: Instant) -> Option<f32> {
        self.blocked += blocked;
        let elapsed = now.saturating_duration_since(self.window_start);
        if elapsed < WINDOW {
            return None;
        }
        let share = self.blocked.as_secs_f32() / elapsed.as_secs_f32();
        self.window_start = now;
        self.blocked = Duration::ZERO;

        let before = self.factor;
        if share > CONGESTED {
            self.calm = 0;
            self.factor = (self.factor * 0.7).max(MIN_FACTOR);
        } else if share < CALM {
            self.calm += 1;
            if self.calm >= CALM_WINDOWS {
                self.calm = 0;
                self.factor = (self.factor * 1.2).min(1.0);
            }
        } else {
            // In between: the current rate fits; hold it.
            self.calm = 0;
        }
        (self.factor != before).then_some(self.factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds one window in which sending blocked for `share` of the time.
    fn window(c: &mut Congestion, now: &mut Instant, share: f32) -> Option<f32> {
        *now += WINDOW;
        c.record(WINDOW.mul_f32(share), *now)
    }

    #[test]
    fn backs_off_and_recovers() {
        let mut now = Instant::now();
        let mut c = Congestion::new(now);
        assert_eq!(window(&mut c, &mut now, 0.6), Some(0.7));
        assert!(window(&mut c, &mut now, 0.6).unwrap() < 0.5);
        for _ in 0..10 {
            window(&mut c, &mut now, 0.9);
        }
        assert_eq!(c.factor(), MIN_FACTOR, "never below the floor");

        // Middle ground holds the rate; three calm windows raise it.
        assert_eq!(window(&mut c, &mut now, 0.1), None);
        assert_eq!(window(&mut c, &mut now, 0.0), None);
        assert_eq!(window(&mut c, &mut now, 0.0), None);
        assert!(window(&mut c, &mut now, 0.0).unwrap() > MIN_FACTOR);
        for _ in 0..60 {
            window(&mut c, &mut now, 0.0);
        }
        assert_eq!(c.factor(), 1.0, "never above the preset");
    }

    #[test]
    fn decides_once_per_window() {
        let mut now = Instant::now();
        let mut c = Congestion::new(now);
        for _ in 0..9 {
            now += Duration::from_millis(100);
            assert_eq!(c.record(Duration::from_millis(90), now), None);
        }
        now += Duration::from_millis(100);
        assert_eq!(c.record(Duration::from_millis(90), now), Some(0.7));
    }
}
