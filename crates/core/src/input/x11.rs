//! Input injection on Linux through the X11 XTest extension.
//!
//! Positions arrive in the display's pixels and are posted in root-window
//! coordinates. Text is typed as `xdotool` does: a spare keycode is mapped to
//! each character's keysym in turn, so it works with any keyboard layout.

use std::collections::HashSet;
use std::time::Duration;

use ctxremote_proto::session::{InputEvent, MouseButton};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Keysym, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::CURRENT_TIME;

use crate::capture::x11::Screen;
use crate::capture::Display;
use crate::keymap::x11_keycode;

/// One wheel notch in Windows units.
const NOTCH: i32 = 120;
/// Longer texts are cut; typing is meant for passwords and short snippets.
const MAX_TYPED_CHARS: usize = 4096;
/// Time for programs to pick up a changed key mapping, before and after a batch.
const REMAP_PAUSE: Duration = Duration::from_millis(20);
/// Keycodes borrowed at most at once for typing.
const MAX_SPARE: usize = 32;

pub struct Injector {
    screen: Option<Screen>,
    origin: (i32, i32),
    size: (i32, i32),
    keys: HashSet<String>,
    buttons: HashSet<MouseButton>,
    /// Wheel movement below a whole notch, kept for the next event.
    wheel_rest: (i32, i32),
}

impl Injector {
    pub fn new(display: &Display) -> Self {
        let screen = Screen::open().inspect_err(|e| tracing::warn!("Eingabe nicht möglich: {e:#}")).ok();
        let mut injector = Self {
            screen,
            origin: (0, 0),
            size: (1, 1),
            keys: HashSet::new(),
            buttons: HashSet::new(),
            wheel_rest: (0, 0),
        };
        injector.set_display(display);
        injector
    }

    pub fn set_display(&mut self, display: &Display) {
        self.origin = (display.left, display.top);
        self.size = (display.width.max(1) as i32, display.height.max(1) as i32);
    }

    pub fn apply(&mut self, event: &InputEvent) {
        match event {
            InputEvent::MouseMove { x, y } => {
                let x = self.origin.0 + (*x as i32).clamp(0, self.size.0 - 1);
                let y = self.origin.1 + (*y as i32).clamp(0, self.size.1 - 1);
                self.fake(MOTION_NOTIFY_EVENT, 0, x as i16, y as i16);
            }
            InputEvent::MouseButton { button, down } => {
                if *down {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
                self.button(*button, *down);
            }
            InputEvent::Wheel { dx, dy } => self.wheel(*dx, *dy),
            InputEvent::Key { code, down } => {
                if *down {
                    self.keys.insert(code.clone());
                } else {
                    self.keys.remove(code);
                }
                if let Some(keycode) = x11_keycode(code) {
                    self.fake(if *down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, keycode, 0, 0);
                } else {
                    tracing::debug!(code, "unbekannte Taste");
                }
            }
            InputEvent::ReleaseAll => self.release_all(),
            InputEvent::Text(text) => self.type_text(text),
        }
        self.flush();
    }

    pub fn release_all(&mut self) {
        for code in std::mem::take(&mut self.keys) {
            if let Some(keycode) = x11_keycode(&code) {
                self.fake(KEY_RELEASE_EVENT, keycode, 0, 0);
            }
        }
        for button in std::mem::take(&mut self.buttons) {
            self.button(button, false);
        }
        self.flush();
    }

    fn button(&self, button: MouseButton, down: bool) {
        let number = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
            MouseButton::Back => 8,
            MouseButton::Forward => 9,
        };
        self.fake(if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, number, 0, 0);
    }

    /// X scrolls by clicks of buttons 4 to 7, one per notch.
    fn wheel(&mut self, dx: i32, dy: i32) {
        let (mut rx, mut ry) = self.wheel_rest;
        rx += dx;
        ry += dy;
        let (nx, ny) = (rx / NOTCH, ry / NOTCH);
        self.wheel_rest = (rx - nx * NOTCH, ry - ny * NOTCH);
        // Positive is up and left, as on Windows.
        let clicks = [(ny, 4u8, 5u8), (nx, 6, 7)];
        for (count, positive, negative) in clicks {
            let button = if count > 0 { positive } else { negative };
            for _ in 0..count.unsigned_abs().min(20) {
                self.fake(BUTTON_PRESS_EVENT, button, 0, 0);
                self.fake(BUTTON_RELEASE_EVENT, button, 0, 0);
            }
        }
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        if let Some(screen) = &self.screen {
            let root = if kind == MOTION_NOTIFY_EVENT { screen.root } else { x11rb::NONE };
            let _ = screen.conn.xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0);
        }
    }

    fn flush(&self) {
        if let Some(screen) = &self.screen {
            let _ = screen.conn.flush();
        }
    }

    /// Types the text as characters, whatever the keyboard layout.
    fn type_text(&mut self, text: &str) {
        let Some(screen) = &self.screen else { return };
        let conn = &screen.conn;
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let Some(mapping) = conn
            .get_keyboard_mapping(min, max - min + 1)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        let per = mapping.keysyms_per_keycode as usize;
        // Keycodes without symbols, each borrowed for one character of a batch.
        let spare: Vec<u8> = (0..=(max - min) as usize)
            .rev()
            .filter(|&i| mapping.keysyms[i * per..(i + 1) * per].iter().all(|&s| s == 0))
            .map(|i| min + i as u8)
            .take(MAX_SPARE)
            .collect();
        if spare.is_empty() {
            tracing::warn!("Keine freie Taste zum Eintippen");
            return;
        }
        let map = |code: u8, sym: Keysym| {
            let _ = conn.change_keyboard_mapping(1, code, per as u8, &vec![sym; per]);
        };
        // Characters are mapped a batch at a time and the mapping holds until
        // well after the batch is typed: a program that reads the mapping a
        // little late still finds the right character.
        let mut batch: Vec<(char, u8)> = Vec::new();
        let flush_batch = |batch: &mut Vec<(char, u8)>| {
            if batch.is_empty() {
                return;
            }
            let _ = conn.sync();
            std::thread::sleep(REMAP_PAUSE);
            for &(c, code) in batch.iter() {
                match c {
                    '\n' => self.tap("Enter"),
                    '\t' => self.tap("Tab"),
                    _ => {
                        self.fake(KEY_PRESS_EVENT, code, 0, 0);
                        self.fake(KEY_RELEASE_EVENT, code, 0, 0);
                    }
                }
            }
            let _ = conn.sync();
            std::thread::sleep(REMAP_PAUSE);
            batch.clear();
        };
        let mut used = 0;
        for c in text.chars().take(MAX_TYPED_CHARS).filter(|&c| c != '\r') {
            // Real keys for line breaks and tabs, so programs that look at keycodes see them.
            if c == '\n' || c == '\t' {
                batch.push((c, 0));
                continue;
            }
            let code = match batch.iter().find(|(b, _)| *b == c) {
                Some(&(_, code)) => code,
                None => {
                    if used == spare.len() {
                        flush_batch(&mut batch);
                        used = 0;
                    }
                    let code = spare[used];
                    used += 1;
                    map(code, keysym_for(c));
                    code
                }
            };
            batch.push((c, code));
        }
        flush_batch(&mut batch);
        for &code in &spare {
            map(code, 0);
        }
        let _ = conn.sync();
    }

    fn tap(&self, code: &str) {
        if let Some(keycode) = x11_keycode(code) {
            self.fake(KEY_PRESS_EVENT, keycode, 0, 0);
            self.fake(KEY_RELEASE_EVENT, keycode, 0, 0);
        }
    }
}

trait Sync {
    fn sync(&self) -> Result<(), x11rb::errors::ReplyError>;
}

impl<C: Connection> Sync for C {
    /// Waits until the server has handled everything sent so far.
    fn sync(&self) -> Result<(), x11rb::errors::ReplyError> {
        self.get_input_focus()?.reply().map(drop)
    }
}

/// Latin-1 characters are their own keysyms; everything else is Unicode.
fn keysym_for(c: char) -> Keysym {
    let cp = c as u32;
    if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) {
        cp
    } else {
        0x0100_0000 | cp
    }
}

#[cfg(test)]
mod tests {
    use super::keysym_for;

    #[test]
    fn keysyms() {
        assert_eq!(keysym_for('a'), 0x61);
        assert_eq!(keysym_for('ü'), 0xfc);
        assert_eq!(keysym_for('€'), 0x0100_20ac);
        assert_eq!(keysym_for('😀'), 0x0101_f600);
    }
}
