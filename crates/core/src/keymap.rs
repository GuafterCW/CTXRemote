//! Maps DOM `KeyboardEvent.code` values to PC/AT Set-1 scancodes.

/// Physical key position as a Set-1 scancode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scancode {
    pub code: u16,
    pub extended: bool,
}

const fn sc(code: u16) -> Option<Scancode> {
    Some(Scancode { code, extended: false })
}

/// E0-prefixed key (`KEYEVENTF_EXTENDEDKEY` on injection).
const fn ext(code: u16) -> Option<Scancode> {
    Some(Scancode { code, extended: true })
}

/// Maps a DOM `KeyboardEvent.code` value to its Set-1 scancode.
pub fn scancode_for(code: &str) -> Option<Scancode> {
    match code {
        "Escape" => sc(0x01),
        "Digit1" => sc(0x02),
        "Digit2" => sc(0x03),
        "Digit3" => sc(0x04),
        "Digit4" => sc(0x05),
        "Digit5" => sc(0x06),
        "Digit6" => sc(0x07),
        "Digit7" => sc(0x08),
        "Digit8" => sc(0x09),
        "Digit9" => sc(0x0A),
        "Digit0" => sc(0x0B),
        "Minus" => sc(0x0C),
        "Equal" => sc(0x0D),
        "Backspace" => sc(0x0E),
        "Tab" => sc(0x0F),
        "KeyQ" => sc(0x10),
        "KeyW" => sc(0x11),
        "KeyE" => sc(0x12),
        "KeyR" => sc(0x13),
        "KeyT" => sc(0x14),
        "KeyY" => sc(0x15),
        "KeyU" => sc(0x16),
        "KeyI" => sc(0x17),
        "KeyO" => sc(0x18),
        "KeyP" => sc(0x19),
        "BracketLeft" => sc(0x1A),
        "BracketRight" => sc(0x1B),
        "Enter" => sc(0x1C),
        "ControlLeft" => sc(0x1D),
        "KeyA" => sc(0x1E),
        "KeyS" => sc(0x1F),
        "KeyD" => sc(0x20),
        "KeyF" => sc(0x21),
        "KeyG" => sc(0x22),
        "KeyH" => sc(0x23),
        "KeyJ" => sc(0x24),
        "KeyK" => sc(0x25),
        "KeyL" => sc(0x26),
        "Semicolon" => sc(0x27),
        "Quote" => sc(0x28),
        "Backquote" => sc(0x29),
        "ShiftLeft" => sc(0x2A),
        "Backslash" => sc(0x2B),
        "KeyZ" => sc(0x2C),
        "KeyX" => sc(0x2D),
        "KeyC" => sc(0x2E),
        "KeyV" => sc(0x2F),
        "KeyB" => sc(0x30),
        "KeyN" => sc(0x31),
        "KeyM" => sc(0x32),
        "Comma" => sc(0x33),
        "Period" => sc(0x34),
        "Slash" => sc(0x35),
        "ShiftRight" => sc(0x36),
        "NumpadMultiply" => sc(0x37),
        "AltLeft" => sc(0x38),
        "Space" => sc(0x39),
        "CapsLock" => sc(0x3A),
        "F1" => sc(0x3B),
        "F2" => sc(0x3C),
        "F3" => sc(0x3D),
        "F4" => sc(0x3E),
        "F5" => sc(0x3F),
        "F6" => sc(0x40),
        "F7" => sc(0x41),
        "F8" => sc(0x42),
        "F9" => sc(0x43),
        "F10" => sc(0x44),
        // NumLock is E0 45 so it is distinguishable from Pause (plain 45).
        "NumLock" => ext(0x45),
        "ScrollLock" => sc(0x46),
        "Numpad7" => sc(0x47),
        "Numpad8" => sc(0x48),
        "Numpad9" => sc(0x49),
        "NumpadSubtract" => sc(0x4A),
        "Numpad4" => sc(0x4B),
        "Numpad5" => sc(0x4C),
        "Numpad6" => sc(0x4D),
        "NumpadAdd" => sc(0x4E),
        "Numpad1" => sc(0x4F),
        "Numpad2" => sc(0x50),
        "Numpad3" => sc(0x51),
        "Numpad0" => sc(0x52),
        "NumpadDecimal" => sc(0x53),
        "IntlBackslash" => sc(0x56),
        "F11" => sc(0x57),
        "F12" => sc(0x58),
        "NumpadEqual" => sc(0x59),
        "F13" => sc(0x64),
        "F14" => sc(0x65),
        "F15" => sc(0x66),
        "F16" => sc(0x67),
        "F17" => sc(0x68),
        "F18" => sc(0x69),
        "F19" => sc(0x6A),
        "F20" => sc(0x6B),
        "F21" => sc(0x6C),
        "F22" => sc(0x6D),
        "F23" => sc(0x6E),
        "F24" => sc(0x76),
        "KanaMode" => sc(0x70),
        "Lang2" => sc(0x71), // Hanja
        "Lang1" => sc(0x72), // Hangul
        "IntlRo" => sc(0x73),
        "Convert" => sc(0x79),
        "NonConvert" => sc(0x7B),
        "IntlYen" => sc(0x7D),
        "NumpadComma" => sc(0x7E),

        // Pause really sends E1 1D 45 9D C5; the plain 0x45 scancode is
        // sufficient for injection.
        "Pause" => sc(0x45),

        // Extended (E0) keys.
        "NumpadEnter" => ext(0x1C),
        "ControlRight" => ext(0x1D),
        "NumpadDivide" => ext(0x35),
        "PrintScreen" => ext(0x37),
        "AltRight" => ext(0x38),
        "Home" => ext(0x47),
        "ArrowUp" => ext(0x48),
        "PageUp" => ext(0x49),
        "ArrowLeft" => ext(0x4B),
        "ArrowRight" => ext(0x4D),
        "End" => ext(0x4F),
        "ArrowDown" => ext(0x50),
        "PageDown" => ext(0x51),
        "Insert" => ext(0x52),
        "Delete" => ext(0x53),
        "MetaLeft" => ext(0x5B),
        "MetaRight" => ext(0x5C),
        "ContextMenu" => ext(0x5D),
        _ => None,
    }
}

/// Maps a DOM `KeyboardEvent.code` value to a macOS virtual key code
/// (`kVK_*`), for a Mac host. Ctrl from the viewer becomes Cmd there and the
/// Windows key becomes Ctrl, so Ctrl+C copies on the Mac as it does on a PC;
/// a Mac viewer sends Cmd as Ctrl, so it arrives as Cmd again.
pub fn mac_keycode(code: &str) -> Option<u16> {
    Some(match code {
        "KeyA" => 0x00,
        "KeyS" => 0x01,
        "KeyD" => 0x02,
        "KeyF" => 0x03,
        "KeyH" => 0x04,
        "KeyG" => 0x05,
        "KeyZ" => 0x06,
        "KeyX" => 0x07,
        "KeyC" => 0x08,
        "KeyV" => 0x09,
        "IntlBackslash" => 0x0A,
        "KeyB" => 0x0B,
        "KeyQ" => 0x0C,
        "KeyW" => 0x0D,
        "KeyE" => 0x0E,
        "KeyR" => 0x0F,
        "KeyY" => 0x10,
        "KeyT" => 0x11,
        "Digit1" => 0x12,
        "Digit2" => 0x13,
        "Digit3" => 0x14,
        "Digit4" => 0x15,
        "Digit6" => 0x16,
        "Digit5" => 0x17,
        "Equal" => 0x18,
        "Digit9" => 0x19,
        "Digit7" => 0x1A,
        "Minus" => 0x1B,
        "Digit8" => 0x1C,
        "Digit0" => 0x1D,
        "BracketRight" => 0x1E,
        "KeyO" => 0x1F,
        "KeyU" => 0x20,
        "BracketLeft" => 0x21,
        "KeyI" => 0x22,
        "KeyP" => 0x23,
        "Enter" => 0x24,
        "KeyL" => 0x25,
        "KeyJ" => 0x26,
        "Quote" => 0x27,
        "KeyK" => 0x28,
        "Semicolon" => 0x29,
        "Backslash" => 0x2A,
        "Comma" => 0x2B,
        "Slash" => 0x2C,
        "KeyN" => 0x2D,
        "KeyM" => 0x2E,
        "Period" => 0x2F,
        "Tab" => 0x30,
        "Space" => 0x31,
        "Backquote" => 0x32,
        "Backspace" => 0x33,
        "Escape" => 0x35,
        "ControlRight" => 0x36,
        "ControlLeft" => 0x37,
        "ShiftLeft" => 0x38,
        "CapsLock" => 0x39,
        "AltLeft" => 0x3A,
        "MetaLeft" => 0x3B,
        "ShiftRight" => 0x3C,
        "AltRight" => 0x3D,
        "MetaRight" => 0x3E,
        "F17" => 0x40,
        "NumpadDecimal" => 0x41,
        "NumpadMultiply" => 0x43,
        "NumpadAdd" => 0x45,
        "NumLock" => 0x47,
        "NumpadDivide" => 0x4B,
        "NumpadEnter" => 0x4C,
        "NumpadSubtract" => 0x4E,
        "F18" => 0x4F,
        "F19" => 0x50,
        "NumpadEqual" => 0x51,
        "Numpad0" => 0x52,
        "Numpad1" => 0x53,
        "Numpad2" => 0x54,
        "Numpad3" => 0x55,
        "Numpad4" => 0x56,
        "Numpad5" => 0x57,
        "Numpad6" => 0x58,
        "Numpad7" => 0x59,
        "F20" => 0x5A,
        "Numpad8" => 0x5B,
        "Numpad9" => 0x5C,
        "IntlYen" => 0x5D,
        "IntlRo" => 0x5E,
        "NumpadComma" => 0x5F,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F3" => 0x63,
        "F8" => 0x64,
        "F9" => 0x65,
        "Lang2" => 0x66,
        "F11" => 0x67,
        "Lang1" => 0x68,
        // A Mac has no print, scroll lock or pause keys; F13–F15 sit there.
        "F13" | "PrintScreen" => 0x69,
        "F16" => 0x6A,
        "F14" | "ScrollLock" => 0x6B,
        "F10" => 0x6D,
        "ContextMenu" => 0x6E,
        "F12" => 0x6F,
        "F15" | "Pause" => 0x71,
        "Insert" => 0x72,
        "Home" => 0x73,
        "PageUp" => 0x74,
        "Delete" => 0x75,
        "F4" => 0x76,
        "End" => 0x77,
        "F2" => 0x78,
        "PageDown" => 0x79,
        "F1" => 0x7A,
        "ArrowLeft" => 0x7B,
        "ArrowRight" => 0x7C,
        "ArrowDown" => 0x7D,
        "ArrowUp" => 0x7E,
        _ => return None,
    })
}

/// Maps a DOM `KeyboardEvent.code` value to an X11 keycode (the Linux evdev
/// code plus 8), for a Linux host.
pub fn x11_keycode(code: &str) -> Option<u8> {
    let evdev: u16 = match code {
        // Where the Set-1 code differs from evdev or is Windows-specific.
        "NumLock" => 69,
        "Pause" => 119,
        "NumpadEqual" => 117,
        "F13" => 183,
        "F14" => 184,
        "F15" => 185,
        "F16" => 186,
        "F17" => 187,
        "F18" => 188,
        "F19" => 189,
        "F20" => 190,
        "F21" => 191,
        "F22" => 192,
        "F23" => 193,
        "F24" => 194,
        "KanaMode" => 93,
        "Lang1" => 122,
        "Lang2" => 123,
        "IntlRo" => 89,
        "Convert" => 92,
        "NonConvert" => 94,
        "IntlYen" => 124,
        "NumpadComma" => 121,
        _ => {
            let sc = scancode_for(code)?;
            if !sc.extended {
                // Up to F12, evdev numbers are the Set-1 codes.
                if sc.code > 0x58 {
                    return None;
                }
                sc.code
            } else {
                match sc.code {
                    0x1C => 96,  // NumpadEnter
                    0x1D => 97,  // ControlRight
                    0x35 => 98,  // NumpadDivide
                    0x37 => 99,  // PrintScreen
                    0x38 => 100, // AltRight
                    0x47 => 102, // Home
                    0x48 => 103, // ArrowUp
                    0x49 => 104, // PageUp
                    0x4B => 105, // ArrowLeft
                    0x4D => 106, // ArrowRight
                    0x4F => 107, // End
                    0x50 => 108, // ArrowDown
                    0x51 => 109, // PageDown
                    0x52 => 110, // Insert
                    0x53 => 111, // Delete
                    0x5B => 125, // MetaLeft
                    0x5C => 126, // MetaRight
                    0x5D => 127, // ContextMenu
                    _ => return None,
                }
            }
        }
    };
    u8::try_from(evdev + 8).ok()
}

#[cfg(test)]
mod tests {
    use super::x11_keycode;

    #[test]
    fn x11_keycodes() {
        // Values as `xev` reports them on a PC keyboard.
        assert_eq!(x11_keycode("Escape"), Some(9));
        assert_eq!(x11_keycode("KeyA"), Some(38));
        assert_eq!(x11_keycode("Enter"), Some(36));
        assert_eq!(x11_keycode("ShiftLeft"), Some(50));
        assert_eq!(x11_keycode("F12"), Some(96));
        assert_eq!(x11_keycode("IntlBackslash"), Some(94));
        assert_eq!(x11_keycode("ControlRight"), Some(105));
        assert_eq!(x11_keycode("ArrowUp"), Some(111));
        assert_eq!(x11_keycode("Delete"), Some(119));
        assert_eq!(x11_keycode("MetaLeft"), Some(133));
        assert_eq!(x11_keycode("NumLock"), Some(77));
        assert_eq!(x11_keycode("Pause"), Some(127));
        assert_eq!(x11_keycode("NoSuchKey"), None);
    }

    use super::*;

    #[test]
    fn mac_samples() {
        assert_eq!(mac_keycode("KeyA"), Some(0x00));
        assert_eq!(mac_keycode("KeyC"), Some(0x08));
        assert_eq!(mac_keycode("ControlLeft"), Some(0x37), "Strg wird Cmd");
        assert_eq!(mac_keycode("MetaLeft"), Some(0x3B), "Windows-Taste wird Ctrl");
        assert_eq!(mac_keycode("ArrowUp"), Some(0x7E));
        assert_eq!(mac_keycode("Bogus"), None);
        // Every key the PC map knows has a place on a Mac, too, except a few
        // Japanese and extra function keys.
        let missing: Vec<&str> = ["Escape", "Backspace", "Enter", "Tab", "Delete", "Home", "End", "F1", "F12"]
            .into_iter()
            .filter(|c| mac_keycode(c).is_none())
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn samples() {
        assert_eq!(scancode_for("KeyA"), Some(Scancode { code: 0x1E, extended: false }));
        assert_eq!(scancode_for("ArrowUp"), Some(Scancode { code: 0x48, extended: true }));
        assert_eq!(scancode_for("NumpadEnter"), Some(Scancode { code: 0x1C, extended: true }));
        assert_eq!(scancode_for("ControlRight"), Some(Scancode { code: 0x1D, extended: true }));
        assert_eq!(scancode_for("F24"), Some(Scancode { code: 0x76, extended: false }));
        assert_eq!(scancode_for("Bogus"), None);
    }
}
