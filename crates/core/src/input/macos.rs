//! Input injection on macOS through `CGEvent`.
//!
//! Needs the "Accessibility" permission (System Settings → Privacy &
//! Security); without it macOS drops the events silently. Positions arrive
//! in the display's pixels and are posted in points.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use ctxremote_proto::session::{InputEvent, MouseButton};

use crate::capture::Display;
use crate::keymap::mac_keycode;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

/// Two clicks within this time and distance make a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f64 = 4.0;
/// One wheel notch (120 Windows units) scrolls this many lines.
const LINES_PER_NOTCH: f64 = 3.0;

/// Whether this app may control the computer; shows macOS' prompt (with a
/// button to the settings) when `ask` is set.
pub fn accessibility_permission(ask: bool) -> bool {
    let options = CFDictionary::from_CFType_pairs(&[(
        // SAFETY: a framework constant, valid for the process lifetime.
        unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) },
        if ask { CFBoolean::true_value() } else { CFBoolean::false_value() },
    )]);
    // SAFETY: a valid dictionary for the call's duration.
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const _) }
}

pub struct Injector {
    index: u8,
    /// The display's top-left corner and size in points, and pixels per point.
    origin: (f64, f64),
    size: (f64, f64),
    scale: f64,
    position: CGPoint,
    keys: HashSet<String>,
    buttons: HashSet<MouseButton>,
    flags: CGEventFlags,
    /// The last press, for double and triple clicks.
    last_click: Option<(Instant, MouseButton, CGPoint, i64)>,
    /// Wheel movement below a whole line, kept for the next event.
    wheel_rest: (f64, f64),
}

impl Injector {
    pub fn new(display: &Display) -> Self {
        // Asks once per start; without it nothing would happen at all.
        accessibility_permission(true);
        let mut injector = Self {
            index: display.index,
            origin: (0.0, 0.0),
            size: (1.0, 1.0),
            scale: 1.0,
            position: CGPoint::new(0.0, 0.0),
            keys: HashSet::new(),
            buttons: HashSet::new(),
            flags: CGEventFlags::empty(),
            last_click: None,
            wheel_rest: (0.0, 0.0),
        };
        injector.set_display(display);
        injector
    }

    pub fn set_display(&mut self, display: &Display) {
        self.index = display.index;
        if let Some((_, bounds)) = crate::capture::display_geometry(display.index) {
            self.origin = (bounds.origin.x, bounds.origin.y);
            self.size = (bounds.size.width.max(1.0), bounds.size.height.max(1.0));
            self.scale = (display.width as f64 / self.size.0).max(0.1);
        }
    }

    pub fn apply(&mut self, event: &InputEvent) {
        match event {
            InputEvent::MouseMove { x, y } => {
                let x = (*x as f64 / self.scale).clamp(0.0, self.size.0 - 1.0);
                let y = (*y as f64 / self.scale).clamp(0.0, self.size.1 - 1.0);
                self.position = CGPoint::new(self.origin.0 + x, self.origin.1 + y);
                // With a button held, macOS expects drags, or nothing moves along.
                let (kind, button, number) = if self.buttons.contains(&MouseButton::Left) {
                    (CGEventType::LeftMouseDragged, CGMouseButton::Left, None)
                } else if self.buttons.contains(&MouseButton::Right) {
                    (CGEventType::RightMouseDragged, CGMouseButton::Right, None)
                } else if self.buttons.contains(&MouseButton::Middle) {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center, Some(2))
                } else if self.buttons.contains(&MouseButton::Back) {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center, Some(3))
                } else if self.buttons.contains(&MouseButton::Forward) {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center, Some(4))
                } else {
                    (CGEventType::MouseMoved, CGMouseButton::Left, None)
                };
                self.mouse(kind, button, number, None);
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
                self.key(code, *down);
            }
            InputEvent::ReleaseAll => self.release_all(),
            InputEvent::Text(text) => type_text(text),
        }
    }

    pub fn release_all(&mut self) {
        for code in std::mem::take(&mut self.keys) {
            self.key(&code, false);
        }
        for button in std::mem::take(&mut self.buttons) {
            self.button(button, false);
        }
        self.flags = CGEventFlags::empty();
    }

    fn button(&mut self, button: MouseButton, down: bool) {
        let (kind, cg, number) = match (button, down) {
            (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left, None),
            (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left, None),
            (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right, None),
            (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right, None),
            (MouseButton::Middle, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center, Some(2)),
            (MouseButton::Middle, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center, Some(2)),
            (MouseButton::Back, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center, Some(3)),
            (MouseButton::Back, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center, Some(3)),
            (MouseButton::Forward, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center, Some(4)),
            (MouseButton::Forward, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center, Some(4)),
        };
        // macOS counts clicks itself only for real mice: double clicks need the count.
        let count = if down {
            let count = match self.last_click {
                Some((at, last, point, count))
                    if last == button
                        && at.elapsed() < DOUBLE_CLICK
                        && (point.x - self.position.x).abs() < DOUBLE_CLICK_DISTANCE
                        && (point.y - self.position.y).abs() < DOUBLE_CLICK_DISTANCE =>
                {
                    count + 1
                }
                _ => 1,
            };
            self.last_click = Some((Instant::now(), button, self.position, count));
            count
        } else {
            self.last_click.map_or(1, |(_, _, _, count)| count)
        };
        self.mouse(kind, cg, number, Some(count));
    }

    fn mouse(&self, kind: CGEventType, button: CGMouseButton, number: Option<i64>, clicks: Option<i64>) {
        let Some(source) = source() else { return };
        let Ok(event) = CGEvent::new_mouse_event(source, kind, self.position, button) else { return };
        if let Some(number) = number {
            event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, number);
        }
        if let Some(clicks) = clicks {
            event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, clicks);
        }
        event.set_flags(self.flags);
        event.post(CGEventTapLocation::HID);
    }

    fn wheel(&mut self, dx: i32, dy: i32) {
        let lines = |delta: i32, rest: &mut f64| {
            *rest += delta as f64 / 120.0 * LINES_PER_NOTCH;
            let whole = rest.trunc();
            *rest -= whole;
            whole as i32
        };
        let (mut rx, mut ry) = self.wheel_rest;
        let (x, y) = (lines(dx, &mut rx), lines(dy, &mut ry));
        self.wheel_rest = (rx, ry);
        if x == 0 && y == 0 {
            return;
        }
        let Some(source) = source() else { return };
        // Line units (1); Windows and macOS agree that positive scrolls up and left.
        if let Ok(event) = CGEvent::new_scroll_event(source, 1, 2, y, -x, 0) {
            event.set_flags(self.flags);
            event.post(CGEventTapLocation::HID);
        }
    }

    fn key(&mut self, code: &str, down: bool) {
        let Some(keycode) = mac_keycode(code) else {
            tracing::debug!(code, "unbekannte Taste");
            return;
        };
        if let Some(flag) = modifier(code) {
            self.flags.set(flag, down);
        }
        let Some(source) = source() else { return };
        if let Ok(event) = CGEvent::new_keyboard_event(source, keycode, down) {
            event.set_flags(self.flags);
            event.post(CGEventTapLocation::HID);
        }
    }
}

/// The flag a modifier key sets (Ctrl from the viewer is Cmd here, see `mac_keycode`).
fn modifier(code: &str) -> Option<CGEventFlags> {
    Some(match code {
        "ShiftLeft" | "ShiftRight" => CGEventFlags::CGEventFlagShift,
        "ControlLeft" | "ControlRight" => CGEventFlags::CGEventFlagCommand,
        "MetaLeft" | "MetaRight" => CGEventFlags::CGEventFlagControl,
        "AltLeft" | "AltRight" => CGEventFlags::CGEventFlagAlternate,
        _ => return None,
    })
}

fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

/// Longer texts are cut; typing is meant for passwords and short snippets.
const MAX_TYPED_CHARS: usize = 4096;

/// Types the text as characters, whatever the Mac's keyboard layout.
fn type_text(text: &str) {
    // macOS takes at most 20 UTF-16 units per event; pairs stay together.
    let mut chunks: Vec<Vec<u16>> = vec![Vec::new()];
    for c in text.chars().take(MAX_TYPED_CHARS).filter(|c| *c != '\r') {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        if chunks.last().is_some_and(|last| last.len() + units.len() > 20) {
            chunks.push(Vec::new());
        }
        chunks.last_mut().expect("never empty").extend_from_slice(units);
    }
    for chunk in chunks.iter().filter(|c| !c.is_empty()) {
        for down in [true, false] {
            let Some(source) = source() else { return };
            if let Ok(event) = CGEvent::new_keyboard_event(source, 0, down) {
                event.set_string_from_utf16_unchecked(chunk);
                event.post(CGEventTapLocation::HID);
            }
        }
    }
}
