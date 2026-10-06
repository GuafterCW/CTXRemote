//! macOS slows down apps it thinks nobody is looking at ("App Nap"): with
//! the window hidden in the tray, capture and encoding get fewer turns and
//! the picture stutters. While a session runs, it is declared as user
//! initiated and latency critical, which keeps the app at full speed.

use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

/// Held for a session's lifetime; ends the activity when dropped.
pub struct Activity(Retained<ProtocolObject<dyn NSObjectProtocol>>);

// SAFETY: the token is an immutable object that NSProcessInfo only compares;
// beginning and ending activities is thread-safe.
unsafe impl Send for Activity {}
unsafe impl Sync for Activity {}

impl Activity {
    pub fn begin(reason: &str) -> Self {
        let options = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep | NSActivityOptions::LatencyCritical;
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &NSString::from_str(reason));
        Self(token)
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        // SAFETY: the token came from `beginActivityWithOptions_reason`.
        unsafe { NSProcessInfo::processInfo().endActivity(&self.0) };
    }
}
