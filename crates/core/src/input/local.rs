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
