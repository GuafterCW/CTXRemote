//! Input injection through `SendInput`.

use std::collections::HashSet;
use std::mem::size_of;

use ctxremote_proto::session::{InputEvent, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY, KEYEVENTF_UNICODE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::capture::Display;
use crate::keymap::scancode_for;

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

pub struct Injector {
    origin: (i32, i32),
    keys: HashSet<String>,
    buttons: HashSet<MouseButton>,
}

impl Injector {
    pub fn new(display: &Display) -> Self {
        Self { origin: (display.left, display.top), keys: HashSet::new(), buttons: HashSet::new() }
    }

    pub fn set_display(&mut self, display: &Display) {
        self.origin = (display.left, display.top);
    }

    pub fn apply(&mut self, event: &InputEvent) {
        match event {
            InputEvent::MouseMove { x, y } => self.move_to(self.origin.0 + x, self.origin.1 + y),
            InputEvent::MouseButton { button, down } => {
                if *down {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
                mouse_button(*button, *down);
            }
            InputEvent::Wheel { dx, dy } => {
                if *dy != 0 {
                    mouse(MOUSEEVENTF_WHEEL, 0, 0, *dy as u32);
                }
                if *dx != 0 {
                    mouse(MOUSEEVENTF_HWHEEL, 0, 0, *dx as u32);
                }
            }
            InputEvent::Key { code, down } => {
                if *down {
                    self.keys.insert(code.clone());
                } else {
                    self.keys.remove(code);
                }
                key(code, *down);
            }
            InputEvent::ReleaseAll => self.release_all(),
            InputEvent::Text(text) => type_text(text),
        }
    }

    /// Lifts everything the viewer still holds, so no key stays stuck after a disconnect.
    pub fn release_all(&mut self) {
        for code in std::mem::take(&mut self.keys) {
            key(&code, false);
        }
        for button in std::mem::take(&mut self.buttons) {
            mouse_button(button, false);
        }
    }

    fn move_to(&self, x: i32, y: i32) {
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
            )
        };
        // Absolute coordinates are normalised to 0..=65535 across the virtual desktop.
        let nx = ((x - vx) as i64 * 65535 / (vw - 1) as i64) as i32;
        let ny = ((y - vy) as i64 * 65535 / (vh - 1) as i64) as i32;
        mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0);
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn mouse_button(button: MouseButton, down: bool) {
    let (flags, data) = match (button, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    };
    mouse(flags, 0, 0, data);
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: u32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    };
    send(&input);
}

fn key(code: &str, down: bool) {
    let Some(sc) = scancode_for(code) else {
        tracing::debug!(code, "unbekannte Taste");
        return;
    };
    let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
    if sc.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: sc.code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send(&input);
}

/// Longer texts are cut; typing is meant for passwords and short snippets.
const MAX_TYPED_CHARS: usize = 4096;

/// Types each character as itself (`KEYEVENTF_UNICODE`), so the host's
/// keyboard layout does not matter. Line breaks and tabs become their keys.
fn type_text(text: &str) {
    let mut last_cr = false;
    for c in text.chars().take(MAX_TYPED_CHARS) {
        match c {
            // \r\n is one line break.
            '\n' if last_cr => {}
            '\r' | '\n' => {
                key("Enter", true);
                key("Enter", false);
            }
            '\t' => {
                key("Tab", true);
                key("Tab", false);
            }
            c if c.is_control() => {}
            c => {
                let mut units = [0u16; 2];
                for &unit in c.encode_utf16(&mut units).iter() {
                    for up in [false, true] {
                        let flags = if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE };
                        let input = INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 {
                                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                            },
                        };
                        // Blocked (UIPI, a changed desktop): the rest would be too,
                        // and each try costs a desktop switch.
                        if !send(&input) {
                            return;
                        }
                    }
                }
            }
        }
        last_cr = c == '\r';
    }
}

/// Whether Windows took the input.
fn send(input: &INPUT) -> bool {
    let inject = || unsafe { SendInput(std::slice::from_ref(input), size_of::<INPUT>() as i32) };
    let mut sent = inject();
    // The input desktop may have changed (lock screen, UAC); follow it and retry once.
    if sent == 0 && crate::desktop::follow_input() {
        sent = inject();
    }
    if sent == 0 {
        // Typically UIPI: the foreground window runs with higher integrity than we do.
        tracing::trace!("SendInput wurde blockiert");
    }
    sent != 0
}
