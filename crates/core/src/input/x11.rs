//! Input injection on Linux through the X11 XTest extension.
//!
//! Positions arrive in the display's pixels and are posted in root-window
//! coordinates. Text is typed as `xdotool` does: a spare keycode is mapped to
//! each character's keysym in turn, so it works with any keyboard layout.
//!
//! The person at the computer keeps the mouse, as on Windows: while they move
//! or click it, the viewer's mouse waits. XInput 2 raw events name the device
//! they came from; XTest's own devices are the viewer's (or another program's).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use ctxremote_proto::session::{InputEvent, MouseButton};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Keysym, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
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
/// How long the person's mouse keeps priority after its last movement.
const LOCAL_PRIORITY: Duration = Duration::from_millis(1000);

/// Milliseconds after [`epoch`] of the person's last own mouse input, 0 for none yet.
static LOCAL_MOUSE: AtomicU64 = AtomicU64::new(0);

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
        watch_local_mouse();
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

    /// Whether the viewer's mouse is being ignored right now, for the viewer
    /// to show a faded pointer.
    pub fn input_paused(&self) -> bool {
        local_mouse_busy()
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
        // As on Windows: dy positive is up (button 4), dx positive is right (button 7).
        let clicks = [(ny, 4u8, 5u8), (nx, 7, 6)];
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

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// The person moved or clicked their own mouse a moment ago. Not in privacy
/// mode, which swallows their input anyway.
fn local_mouse_busy() -> bool {
    if crate::privacy::LOCAL_INPUT_BLOCKED.load(Ordering::Relaxed) {
        return false;
    }
    let last = LOCAL_MOUSE.load(Ordering::Relaxed);
    last != 0 && (epoch().elapsed().as_millis() as u64).saturating_sub(last) < LOCAL_PRIORITY.as_millis() as u64
}

/// Starts, once per process, a thread that notes the person's own mouse input.
fn watch_local_mouse() {
    static START: Once = Once::new();
    START.call_once(|| {
        epoch();
        let spawned = std::thread::Builder::new().name("ctxremote-local-mouse".into()).spawn(|| {
            if let Err(e) = local_mouse_loop() {
                tracing::warn!("Eigene Maus am Gerät nicht erkennbar: {e:#}");
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("Eigene Maus am Gerät nicht erkennbar: {e}");
        }
    });
}

fn local_mouse_loop() -> anyhow::Result<()> {
    use anyhow::Context as _;
    let (conn, number) = x11rb::connect(None).context("keine X11-Sitzung (DISPLAY)")?;
    let root = conn.setup().roots[number].root;
    // Raw events reach the root window whatever grabs other programs hold (XInput 2.1).
    let version = conn.xinput_xi_query_version(2, 2)?.reply().context("XInput 2 fehlt")?;
    anyhow::ensure!((version.major_version, version.minor_version) >= (2, 1), "XInput {}.{} ist zu alt", version.major_version, version.minor_version);
    let mask = xinput::XIEventMask::RAW_MOTION | xinput::XIEventMask::RAW_BUTTON_PRESS;
    conn.xinput_xi_select_events(
        root,
        &[
            xinput::EventMask { deviceid: xinput::DeviceId::from(xinput::Device::ALL_MASTER), mask: vec![mask] },
            // Devices plugged in or out: the list of XTest devices changes.
            xinput::EventMask { deviceid: xinput::DeviceId::from(xinput::Device::ALL), mask: vec![xinput::XIEventMask::HIERARCHY] },
        ],
    )?
    .check()?;
    conn.flush()?;
    let mut injected = xtest_devices(&conn)?;
    loop {
        let source = match conn.wait_for_event()? {
            Event::XinputRawMotion(e) | Event::XinputRawButtonPress(e) => e.sourceid,
            Event::XinputHierarchy(_) => {
                injected = xtest_devices(&conn)?;
                continue;
            }
            _ => continue,
        };
        if !injected.contains(&source) {
            LOCAL_MOUSE.store((epoch().elapsed().as_millis() as u64).max(1), Ordering::Relaxed);
        }
    }
}

/// XTest's devices: the X server names them "… XTEST pointer" and "… XTEST keyboard".
fn xtest_devices(conn: &impl Connection) -> anyhow::Result<HashSet<u16>> {
    let devices = conn.xinput_xi_query_device(xinput::DeviceId::from(xinput::Device::ALL))?.reply()?;
    Ok(devices
        .infos
        .iter()
        .filter(|d| String::from_utf8_lossy(&d.name).contains("XTEST"))
        .map(|d| d.deviceid)
        .collect())
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
    use super::*;

    /// Clicks "the person's own mouse": XTest posts the press for a pointer
    /// that is not one of XTest's (Xvfb has "Xvfb mouse"), as a real mouse would.
    fn local_click(conn: &impl Connection) {
        let devices = conn.xinput_xi_query_device(xinput::DeviceId::from(xinput::Device::ALL)).unwrap().reply().unwrap();
        let mouse = devices
            .infos
            .iter()
            .find(|d| d.type_ == xinput::DeviceType::SLAVE_POINTER && !String::from_utf8_lossy(&d.name).contains("XTEST"))
            .expect("a pointer besides XTest's");
        let id = mouse.deviceid as u8;
        conn.xinput_open_device(id).unwrap().reply().unwrap();
        let first = conn.query_extension(b"XInputExtension").unwrap().reply().unwrap().first_event;
        // XInput 1 event numbers: DeviceButtonPress 3, DeviceButtonRelease 4.
        conn.xtest_fake_input(first + 3, 1, CURRENT_TIME, x11rb::NONE, 0, 0, id).unwrap().check().unwrap();
        conn.xtest_fake_input(first + 4, 1, CURRENT_TIME, x11rb::NONE, 0, 0, id).unwrap().check().unwrap();
    }

    fn pointer(conn: &impl Connection, root: u32) -> (i16, i16) {
        let p = conn.query_pointer(root).unwrap().reply().unwrap();
        (p.root_x, p.root_y)
    }

    fn wait_for(paused: bool, injector: &Injector) -> bool {
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if injector.input_paused() == paused {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Needs an X server (`xvfb-run`); skipped without one.
    #[test]
    fn local_mouse_has_priority() {
        let Ok(screen) = Screen::open() else {
            eprintln!("übersprungen: keine X11-Sitzung");
            return;
        };
        let display = Display { index: 0, name: "test".into(), left: 0, top: 0, width: screen.width as u32, height: screen.height as u32, primary: true };
        let mut injector = Injector::new(&display);
        // Time for the watcher to subscribe.
        std::thread::sleep(Duration::from_millis(300));

        // The viewer's own moves do not count as the person's.
        injector.apply(&InputEvent::MouseMove { x: 100, y: 100 });
        injector.apply(&InputEvent::MouseMove { x: 120, y: 110 });
        std::thread::sleep(Duration::from_millis(200));
        assert!(!injector.input_paused(), "the viewer's mouse paused itself");
        assert_eq!(pointer(&screen.conn, screen.root), (120, 110));

        // The person clicks: the viewer's mouse waits.
        local_click(&screen.conn);
        assert!(wait_for(true, &injector), "the person's click was not noticed");
        injector.apply(&InputEvent::MouseMove { x: 300, y: 200 });
        assert_eq!(pointer(&screen.conn, screen.root), (120, 110), "the viewer moved the mouse meanwhile");

        // A second later the viewer has it again.
        assert!(wait_for(false, &injector), "the pause did not end");
        injector.apply(&InputEvent::MouseMove { x: 300, y: 200 });
        assert_eq!(pointer(&screen.conn, screen.root), (300, 200));
    }

    #[test]
    fn keysyms() {
        assert_eq!(keysym_for('a'), 0x61);
        assert_eq!(keysym_for('ü'), 0xfc);
        assert_eq!(keysym_for('€'), 0x0100_20ac);
        assert_eq!(keysym_for('😀'), 0x0101_f600);
    }
}
