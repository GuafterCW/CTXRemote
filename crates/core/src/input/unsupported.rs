//! Placeholder for platforms without an input backend yet.

use ctxremote_proto::session::InputEvent;

use crate::capture::Display;

pub struct Injector;

impl Injector {
    pub fn new(_: &Display) -> Self {
        Self
    }

    pub fn protect_own_ui(&mut self, _: bool) {}

    pub fn set_display(&mut self, _: &Display) {}

    pub fn apply(&mut self, _: &InputEvent) {}

    /// Whether the viewer's mouse is being ignored; never without a backend.
    pub fn input_paused(&self) -> bool {
        false
    }

    pub fn release_all(&mut self) {}
}
