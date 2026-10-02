//! Placeholder for platforms without a capture backend yet.

use anyhow::{bail, Result};

use super::Display;

pub fn displays() -> Result<Vec<Display>> {
    bail!(super::UNSUPPORTED)
}

pub struct Capturer {
    display: Display,
}

impl Capturer {
    pub fn new(_index: u8) -> Result<Self> {
        bail!(super::UNSUPPORTED)
    }

    pub fn display(&self) -> &Display {
        &self.display
    }

    pub fn next_frame(&mut self, _: u32, _: impl FnOnce(&[u8], usize, u32, u32)) -> Result<bool> {
        bail!(super::UNSUPPORTED)
    }
}
