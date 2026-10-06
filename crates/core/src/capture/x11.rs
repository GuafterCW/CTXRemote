//! Screen capture on Linux through X11.
//!
//! Monitors come from RandR, the picture from MIT-SHM (a shared memory
//! segment the X server writes into; plain `GetImage` where that is missing)
//! and the pointer shape from XFixes. X has no "picture changed" signal
//! without the Damage extension, so each frame is compared with the last one.
//! A Wayland session only shows X11 windows (XWayland) this way.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::CursorShape;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use super::Display;

/// The X server and its root window (all monitors in one coordinate space).
pub(crate) struct Screen {
    pub conn: RustConnection,
    pub root: Window,
    pub width: u16,
    pub height: u16,
}

impl Screen {
    pub fn open() -> Result<Self> {
        let (conn, number) = x11rb::connect(None).context("keine X11-Sitzung (DISPLAY)")?;
        let screen = &conn.setup().roots[number];
        let (root, width, height) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);
        if screen.root_depth < 24 {
            bail!("Farbtiefe {} wird nicht unterstützt", screen.root_depth);
        }
        Ok(Self { conn, root, width, height })
    }

    /// The monitors, or the whole screen as one where RandR knows none.
    pub fn displays(&self) -> Result<Vec<Display>> {
        let monitors = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.monitors)
            .unwrap_or_default();
        let mut list: Vec<Display> = monitors
            .iter()
            .filter(|m| m.width > 0 && m.height > 0)
            .enumerate()
            .map(|(i, m)| Display {
                index: i as u8,
                name: self
                    .conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_else(|| format!("Bildschirm {}", i + 1)),
                left: m.x as i32,
                top: m.y as i32,
                width: m.width as u32,
                height: m.height as u32,
                primary: m.primary,
            })
            .collect();
        if list.is_empty() {
            list.push(Display {
                index: 0,
                name: "Bildschirm".into(),
                left: 0,
                top: 0,
                width: self.width as u32,
                height: self.height as u32,
                primary: true,
            });
        }
        if !list.iter().any(|d| d.primary) {
            list[0].primary = true;
        }
        Ok(list)
    }
}

pub fn displays() -> Result<Vec<Display>> {
    Screen::open()?.displays()
}

/// A shared memory segment the X server fills with the picture.
struct Shared {
    segment: u32,
    data: *mut u8,
    size: usize,
}

impl Shared {
    fn create(conn: &RustConnection, size: usize) -> Result<Self> {
        let segment = conn.generate_id()?;
        // `read_only` is from the server's side: it writes the picture.
        let reply = conn.shm_create_segment(segment, size as u32, false)?.reply()?;
        let fd: std::os::fd::OwnedFd = reply.shm_fd.into();
        // SAFETY: maps the segment the server just created for us, read only.
        let data = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                std::os::fd::AsRawFd::as_raw_fd(&fd),
                0,
            )
        };
        if data == libc::MAP_FAILED {
            let _ = conn.shm_detach(segment);
            bail!("gemeinsamer Speicher nicht verfügbar");
        }
        Ok(Self { segment, data: data.cast(), size })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping lives as long as `self`.
        unsafe { std::slice::from_raw_parts(self.data, self.size) }
    }
}

// The mapping is only read on the capture thread.
unsafe impl Send for Shared {}

pub struct Capturer {
    screen: Screen,
    display: Display,
    shared: Option<Shared>,
    /// The last picture, to tell whether anything changed.
    last: Vec<u8>,
    last_at: Option<Instant>,
    cursor_serial: Option<u32>,
    xfixes: bool,
}

impl Capturer {
    pub fn new(index: u8) -> Result<Self> {
        let screen = Screen::open()?;
        let display = screen
            .displays()?
            .into_iter()
            .find(|d| d.index == index)
            .context("Bildschirm nicht gefunden")?;
        let size = display.width as usize * display.height as usize * 4;
        let shared = match screen.conn.shm_query_version().ok().and_then(|c| c.reply().ok()) {
            Some(_) => Shared::create(&screen.conn, size)
                .inspect_err(|e| tracing::debug!("MIT-SHM nicht nutzbar, langsamer Weg: {e:#}"))
                .ok(),
            None => None,
        };
        let xfixes = screen.conn.xfixes_query_version(4, 0).ok().and_then(|c| c.reply().ok()).is_some();
        Ok(Self { screen, display, shared, last: Vec::new(), last_at: None, cursor_serial: None, xfixes })
    }

    pub fn display(&self) -> &Display {
        &self.display
    }

    /// Waits until `timeout_ms` after the last picture, takes a new one and
    /// hands it to `consume` (BGRA, row pitch, width, height) if it changed.
    pub fn next_frame(&mut self, timeout_ms: u32, consume: impl FnOnce(&[u8], usize, u32, u32)) -> Result<bool> {
        if let Some(at) = self.last_at {
            let due = at + Duration::from_millis(timeout_ms as u64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        self.last_at = Some(Instant::now());
        let d = &self.display;
        let (x, y, w, h) = (d.left as i16, d.top as i16, d.width as u16, d.height as u16);
        let conn = &self.screen.conn;
        let shared_ok = match &self.shared {
            Some(shared) => {
                match conn.shm_get_image(self.screen.root, x, y, w, h, !0, ImageFormat::Z_PIXMAP.into(), shared.segment, 0)?.reply() {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::debug!("MIT-SHM-Bild fehlgeschlagen, langsamer Weg: {e}");
                        false
                    }
                }
            }
            None => false,
        };
        if !shared_ok {
            self.drop_shared();
        }
        let conn = &self.screen.conn;
        let fallback;
        let fresh: &[u8] = match &self.shared {
            Some(shared) => shared.bytes(),
            None => {
                fallback = conn
                    .get_image(ImageFormat::Z_PIXMAP, self.screen.root, x, y, w, h, !0)?
                    .reply()
                    .context("Bild nicht lesbar")?
                    .data;
                &fallback
            }
        };
        let size = w as usize * h as usize * 4;
        if fresh.len() < size {
            bail!("Bild unvollständig");
        }
        let fresh = &fresh[..size];
        if self.last == fresh {
            return Ok(false);
        }
        self.last.clear();
        self.last.extend_from_slice(fresh);
        consume(&self.last, w as usize * 4, w as u32, h as u32);
        Ok(true)
    }

    /// The pointer's shape when it changed (X keeps it out of the picture).
    pub fn take_pointer(&mut self) -> Option<CursorShape> {
        if !self.xfixes {
            return None;
        }
        let image = self.screen.conn.xfixes_get_cursor_image().ok()?.reply().ok()?;
        if self.cursor_serial == Some(image.cursor_serial) {
            return None;
        }
        self.cursor_serial = Some(image.cursor_serial);
        Some(CursorShape {
            width: image.width as u32,
            height: image.height as u32,
            hot_x: image.xhot as u32,
            hot_y: image.yhot as u32,
            rgba: cursor_rgba(&image.cursor_image),
        })
    }
}

impl Capturer {
    fn drop_shared(&mut self) {
        if let Some(shared) = self.shared.take() {
            // SAFETY: unmaps what `Shared::create` mapped, once.
            unsafe { libc::munmap(shared.data.cast(), shared.size) };
            let _ = self.screen.conn.shm_detach(shared.segment);
            let _ = self.screen.conn.flush();
        }
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        self.drop_shared();
    }
}

/// XFixes pointer pixels (premultiplied ARGB in a `u32`) as straight RGBA.
fn cursor_rgba(pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for &p in pixels {
        let a = (p >> 24) as u8;
        let unmul = |c: u32| if a == 0 { 0 } else { ((c & 0xff) * 255 / a as u32).min(255) as u8 };
        out.extend_from_slice(&[unmul(p >> 16), unmul(p >> 8), unmul(p), a]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_pixels_lose_premultiplication() {
        let rgba = cursor_rgba(&[0xff_ff_00_00, 0x80_40_00_00, 0x00_00_00_00]);
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[127, 0, 0, 128]);
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0]);
    }
}
