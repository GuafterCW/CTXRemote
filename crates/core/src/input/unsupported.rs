//! Placeholder for platforms without an input backend yet.

use ctxremote_proto::session::InputEvent;

use crate::capture::Display;

pub struct Injector;

impl Injector {
    pub fn new(_: &Display) -> Self {
        Self
    }

    pub fn set_display(&mut self, _: &Display) {}

    pub fn apply(&mut self, _: &InputEvent) {}

    /// Whether the viewer's mouse is being ignored; only Windows hosts do that yet.
    pub fn input_paused(&self) -> bool {
        false
    }

    pub fn release_all(&mut self) {}
}
