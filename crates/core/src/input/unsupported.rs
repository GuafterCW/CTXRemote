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

    pub fn release_all(&mut self) {}
}
