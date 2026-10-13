//! Input injection through `SendInput`.
//!
//! The person at the device keeps the mouse: while they move it, the
//! viewer's mouse moves and clicks wait (like AnyDesk). A low-level hook
//! notices their mouse by the missing "injected" flag, whatever the screen
//! scaling.
//!
//! In attended sessions the viewer may not operate CTXRemote's own windows
//! (rights, settings, ending the session): the hooks swallow the viewer's
//! clicks on them and keys while one is in front. The viewer's input is told
//! apart from other injected input (an on-screen keyboard) by [`TAG`].

use std::collections::HashSet;
use std::mem::size_of;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Once;

use ctxremote_proto::session::{InputEvent, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY, KEYEVENTF_UNICODE,
};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowThreadProcessId,
    SetWindowsHookExW, WindowFromPoint, GA_ROOT, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WM_XBUTTONUP,
};

use crate::capture::Display;
use crate::keymap::scancode_for;

/// How long the person's mouse keeps priority after its last movement.
const LOCAL_PRIORITY_MS: u64 = 1000;

/// Tick count of the person's last own mouse input, 0 for none yet.
static LOCAL_MOUSE: AtomicU64 = AtomicU64::new(0);

/// Marks the input this module injects (`dwExtraInfo`), so the hooks know
/// the viewer's input from that of other programs.
const TAG: usize = 0x4354_5852; // "CTXR"

/// Injectors whose session protects CTXRemote's own windows.
static PROTECTING: AtomicUsize = AtomicUsize::new(0);
/// Tick count of the viewer's last input swallowed at CTXRemote's windows.
static OWN_UI_HIT: AtomicU64 = AtomicU64::new(0);
/// Mouse buttons whose press was swallowed; their release is swallowed, too.
static HELD_BUTTONS: AtomicU32 = AtomicU32::new(0);
/// Keys (by virtual-key code) whose press was swallowed.
static HELD_KEYS: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

/// CTXRemote's programs with windows (the app and the quick helper).
const OWN_PROGRAMS: [&str; 2] = ["ctxremote.exe", "ctxremote-hilfe.exe"];

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

pub struct Injector {
    origin: (i32, i32),
    keys: HashSet<String>,
    buttons: HashSet<MouseButton>,
    /// Counted in [`PROTECTING`].
    protecting: bool,
}

impl Injector {
    pub fn new(display: &Display) -> Self {
        watch_local_mouse();
        Self { origin: (display.left, display.top), keys: HashSet::new(), buttons: HashSet::new(), protecting: false }
    }

    /// In attended sessions the viewer's input does not reach CTXRemote's own windows.
    pub fn protect_own_ui(&mut self, on: bool) {
        if on != self.protecting {
            self.protecting = on;
            if on {
                PROTECTING.fetch_add(1, Ordering::Relaxed);
            } else {
                PROTECTING.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Whether the viewer's mouse is being ignored right now, for the viewer
    /// to show a faded pointer.
    pub fn input_paused(&self) -> bool {
        let now = unsafe { GetTickCount64() };
        let hit = OWN_UI_HIT.load(Ordering::Relaxed);
        local_mouse_busy() || (self.protecting && hit != 0 && now.saturating_sub(hit) < LOCAL_PRIORITY_MS)
    }

    pub fn set_display(&mut self, display: &Display) {
        self.origin = (display.left, display.top);
    }

    pub fn apply(&mut self, event: &InputEvent) {
        // Releasing stays allowed, so nothing is left held down.
        let pressing = match event {
            InputEvent::MouseMove { .. } | InputEvent::Wheel { .. } => true,
            InputEvent::MouseButton { down, .. } => *down,
            _ => false,
        };
        if pressing && local_mouse_busy() {
            return;
        }
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
        self.protect_own_ui(false);
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
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: TAG },
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
                dwExtraInfo: TAG,
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
                                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: TAG },
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

/// Starts, once per process, the thread whose hook notes the person's own mouse.
fn watch_local_mouse() {
    static START: Once = Once::new();
    START.call_once(|| {
        let started = std::thread::Builder::new().name("local-mouse".into()).spawn(|| {
            // The hook sees the input of the desktop the person uses.
            crate::desktop::follow_input();
            // SAFETY: the hook and its message loop belong to this thread,
            // which lives as long as the process.
            unsafe {
                let module = GetModuleHandleW(None).unwrap_or_default();
                if let Err(e) = SetWindowsHookExW(WH_MOUSE_LL, Some(local_mouse_hook), Some(module.into()), 0) {
                    tracing::warn!("Maus am Gerät nicht erkennbar: {e}");
                    return;
                }
                if let Err(e) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(module.into()), 0) {
                    tracing::warn!("Tastatur des Viewers an CTXRemote-Fenstern nicht sperrbar: {e}");
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            }
        });
        if let Err(e) = started {
            tracing::warn!("Maus am Gerät nicht erkennbar: {e}");
        }
    });
}

unsafe extern "system" fn local_mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_MOUSE_LL, lparam points to an MSLLHOOKSTRUCT.
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if event.flags & LLMHF_INJECTED == 0 {
            LOCAL_MOUSE.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
        } else if event.dwExtraInfo == TAG && PROTECTING.load(Ordering::Relaxed) > 0 && swallow_click(wparam.0 as u32, event) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// A viewer's press or wheel over CTXRemote's own window, or the release of a
/// press that was swallowed.
fn swallow_click(message: u32, event: &MSLLHOOKSTRUCT) -> bool {
    let button = match message {
        WM_LBUTTONDOWN | WM_LBUTTONUP => 1,
        WM_RBUTTONDOWN | WM_RBUTTONUP => 2,
        WM_MBUTTONDOWN | WM_MBUTTONUP => 4,
        // The high word names the X button: 1 or 2.
        WM_XBUTTONDOWN | WM_XBUTTONUP => 8 << ((event.mouseData >> 16) & 1),
        _ => 0,
    };
    match message {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            if !own_window(unsafe { WindowFromPoint(event.pt) }) {
                return false;
            }
            HELD_BUTTONS.fetch_or(button, Ordering::Relaxed);
            OWN_UI_HIT.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
            true
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
            HELD_BUTTONS.fetch_and(!button, Ordering::Relaxed) & button != 0
        }
        _ => false,
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if event.flags.0 & LLKHF_INJECTED.0 != 0
            && event.dwExtraInfo == TAG
            && PROTECTING.load(Ordering::Relaxed) > 0
            && swallow_key(wparam.0 as u32, event.vkCode)
        {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// A viewer's key while CTXRemote's own window is in front, or the release
/// of a key whose press was swallowed.
fn swallow_key(message: u32, vk: u32) -> bool {
    let (word, bit) = ((vk as usize / 64) % 4, 1u64 << (vk % 64));
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if !own_window(unsafe { GetForegroundWindow() }) {
                return false;
            }
            HELD_KEYS[word].fetch_or(bit, Ordering::Relaxed);
            OWN_UI_HIT.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
            true
        }
        WM_KEYUP | WM_SYSKEYUP => HELD_KEYS[word].fetch_and(!bit, Ordering::Relaxed) & bit != 0,
        _ => false,
    }
}

/// Whether `window` belongs to CTXRemote's app or quick helper. Answers per
/// process are kept for a few seconds; hooks must be quick.
fn own_window(window: HWND) -> bool {
    thread_local! {
        static KNOWN: RefCell<HashMap<u32, (bool, u64)>> = RefCell::new(HashMap::new());
    }
    if window.is_invalid() {
        return false;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(GetAncestor(window, GA_ROOT), Some(&mut pid)) };
    if pid == 0 {
        return false;
    }
    let now = unsafe { GetTickCount64() };
    KNOWN.with(|known| {
        let mut known = known.borrow_mut();
        if let Some(&(own, at)) = known.get(&pid) {
            if now.saturating_sub(at) < 5_000 {
                return own;
            }
        }
        let own = process_name(pid).is_some_and(|name| OWN_PROGRAMS.contains(&name.as_str()));
        if known.len() > 256 {
            known.clear();
        }
        known.insert(pid, (own, now));
        own
    })
}

/// The file name of a process's program, in lower case.
fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        Some(path.rsplit('\\').next().unwrap_or(&path).to_lowercase())
    }
}

/// The person moved or clicked their own mouse a moment ago. Not in privacy
/// mode, which swallows their input anyway.
fn local_mouse_busy() -> bool {
    if crate::privacy::LOCAL_INPUT_BLOCKED.load(Ordering::Relaxed) {
        return false;
    }
    let last = LOCAL_MOUSE.load(Ordering::Relaxed);
    last != 0 && unsafe { GetTickCount64() }.saturating_sub(last) < LOCAL_PRIORITY_MS
}
