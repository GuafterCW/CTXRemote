//! When the person at the computer last used their own mouse (Linux and
//! macOS; Windows keeps its own tick count in its hook).
//!
//! The backends call [`note`] for every mouse event that did not come from
//! the viewer; while [`busy`], the viewer's mouse waits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long the person's mouse keeps priority after its last movement.
const PRIORITY: Duration = Duration::from_millis(1000);

/// Milliseconds after [`epoch`] of the person's last own mouse input, 0 for none yet.
static LAST: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// The person moved or clicked their own mouse just now.
pub(super) fn note() {
    LAST.store(now().max(1), Ordering::Relaxed);
}

/// The person moved or clicked their own mouse a moment ago. Not in privacy
/// mode, which swallows their input anyway.
pub(super) fn busy() -> bool {
    if crate::privacy::LOCAL_INPUT_BLOCKED.load(Ordering::Relaxed) {
        return false;
    }
    let last = LAST.load(Ordering::Relaxed);
    last != 0 && now().saturating_sub(last) < PRIORITY.as_millis() as u64
}

/// Viewer events that wait while the person has the mouse. Releasing stays
/// allowed, so nothing is left held down.
pub(super) fn waits(event: &ctxremote_proto::session::InputEvent) -> bool {
    use ctxremote_proto::session::InputEvent;
    let pressing = match event {
        InputEvent::MouseMove { .. } | InputEvent::Wheel { .. } => true,
        InputEvent::MouseButton { down, .. } => *down,
        _ => false,
    };
    pressing && busy()
}

/// In attended sessions the viewer may not operate CTXRemote's own windows
/// (rights, settings, ending the session), as on Windows. The backends post
/// the viewer's input themselves, so they ask before each press where it
/// would land, instead of catching it afterwards.
#[derive(Default)]
pub(super) struct OwnUi {
    pub on: bool,
    /// Presses that were swallowed; their releases are swallowed, too.
    buttons: std::collections::HashSet<ctxremote_proto::session::MouseButton>,
    keys: std::collections::HashSet<String>,
    /// When the viewer last hit CTXRemote's windows, for the faded pointer.
    hit: Option<Instant>,
}

impl OwnUi {
    /// Whether to drop the viewer's `event`. `under_pointer` and `in_front`
    /// tell whether the window under the pointer, or in front, is CTXRemote's;
    /// they are asked only when it matters.
    pub fn swallow(&mut self, event: &ctxremote_proto::session::InputEvent, under_pointer: impl FnOnce() -> bool, in_front: impl FnOnce() -> bool) -> bool {
        use ctxremote_proto::session::InputEvent;
        if !self.on {
            return false;
        }
        let swallowed = match event {
            InputEvent::MouseButton { button, down: true } => {
                let own = under_pointer();
                if own {
                    self.buttons.insert(*button);
                }
                own
            }
            InputEvent::MouseButton { button, down: false } => return self.buttons.remove(button),
            InputEvent::Wheel { .. } => under_pointer(),
            InputEvent::Key { code, down: true } => {
                let own = in_front();
                if own {
                    self.keys.insert(code.clone());
                }
                own
            }
            InputEvent::Key { code, down: false } => return self.keys.remove(code),
            InputEvent::Text(_) => in_front(),
            InputEvent::ReleaseAll => {
                self.buttons.clear();
                self.keys.clear();
                false
            }
            InputEvent::MouseMove { .. } => false,
        };
        if swallowed {
            self.hit = Some(Instant::now());
        }
        swallowed
    }

    /// The viewer hit CTXRemote's windows a moment ago.
    pub fn recently_hit(&self) -> bool {
        self.on && self.hit.is_some_and(|at| at.elapsed() < PRIORITY)
    }
}

#[cfg(test)]
mod tests {
    use super::OwnUi;
    use ctxremote_proto::session::{InputEvent, MouseButton};

    #[test]
    fn own_windows_are_off_limits() {
        let press = |down| InputEvent::MouseButton { button: MouseButton::Left, down };
        let key = |down| InputEvent::Key { code: "KeyA".into(), down };
        let mut ui = OwnUi::default();
        assert!(!ui.swallow(&press(true), || true, || true), "off: everything passes");

        ui.on = true;
        assert!(!ui.swallow(&press(true), || false, || true), "a click elsewhere passes");
        assert!(!ui.swallow(&press(false), || true, || true), "so does its release");
        assert!(!ui.recently_hit());

        assert!(ui.swallow(&press(true), || true, || false), "a click on CTXRemote is dropped");
        assert!(ui.recently_hit());
        assert!(ui.swallow(&press(false), || false, || false), "and its release, wherever");
        assert!(!ui.swallow(&InputEvent::MouseMove { x: 1, y: 1 }, || true, || true), "moving stays allowed");
        assert!(ui.swallow(&InputEvent::Wheel { dx: 0, dy: 120 }, || true, || false));

        assert!(ui.swallow(&key(true), || false, || true), "typing into CTXRemote is dropped");
        assert!(ui.swallow(&key(false), || false, || false));
        assert!(!ui.swallow(&key(true), || true, || false), "typing elsewhere passes");
        assert!(!ui.swallow(&key(false), || true, || true));
        assert!(ui.swallow(&InputEvent::Text("x".into()), || false, || true));
    }
}
