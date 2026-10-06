//! Screen capture on macOS through `CGDisplayStream`, as RustDesk does: the
//! system delivers each new picture of a display as an IOSurface in BGRA,
//! and the newest one is handed to the encoder.
//!
//! Needs the "Screen Recording" permission (System Settings → Privacy &
//! Security). Without it the stream starts but shows only the wallpaper, so
//! the permission is checked first.
//!
//! Positions on a Mac are in points, pictures in pixels: [`Display`] keeps
//! `left`/`top` in points (for input) and `width`/`height` in pixels.

use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use block2::RcBlock;
use core_foundation::base::{CFRelease, CFRetain, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::display::CGDisplay;
use ctxremote_proto::session::CursorShape;

use super::Display;

type IOSurfaceRef = *mut c_void;
type CGDisplayStreamRef = *mut c_void;

/// `'BGRA'`.
const PIXEL_FORMAT_BGRA: i32 = 0x4247_5241;
const FRAME_COMPLETE: i32 = 0;
const FRAME_STOPPED: i32 = 3;
const LOCK_READ_ONLY: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGDisplayStreamShowCursor: CFStringRef;
    static kCGDisplayStreamMinimumFrameTime: CFStringRef;
    static kCGDisplayStreamQueueDepth: CFStringRef;
    fn CGDisplayStreamCreateWithDispatchQueue(
        display: u32,
        output_width: usize,
        output_height: usize,
        pixel_format: i32,
        properties: *const c_void,
        queue: *mut c_void,
        handler: &block2::Block<dyn Fn(i32, u64, IOSurfaceRef, *const c_void)>,
    ) -> CGDisplayStreamRef;
    fn CGDisplayStreamStart(stream: CGDisplayStreamRef) -> i32;
    fn CGDisplayStreamStop(stream: CGDisplayStreamRef) -> i32;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceLock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceIncrementUseCount(surface: IOSurfaceRef);
    fn IOSurfaceDecrementUseCount(surface: IOSurfaceRef);
    fn IOSurfaceUnlock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: IOSurfaceRef) -> *mut c_void;
    fn IOSurfaceGetBytesPerRow(surface: IOSurfaceRef) -> usize;
    fn IOSurfaceGetWidth(surface: IOSurfaceRef) -> usize;
    fn IOSurfaceGetHeight(surface: IOSurfaceRef) -> usize;
}

extern "C" {
    fn dispatch_queue_create(label: *const std::ffi::c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

/// Whether this app may record the screen; asks once (macOS shows the
/// prompt and lists the app in System Settings) when `ask` is set.
pub fn screen_permission(ask: bool) -> bool {
    // SAFETY: plain calls without arguments.
    unsafe { CGPreflightScreenCaptureAccess() || (ask && CGRequestScreenCaptureAccess()) }
}

pub fn displays() -> Result<Vec<Display>> {
    let ids = CGDisplay::active_displays().map_err(|e| anyhow::anyhow!("Bildschirme nicht lesbar ({e})"))?;
    Ok(ids
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let display = CGDisplay::new(id);
            let bounds = display.bounds();
            Display {
                index: index as u8,
                name: if display.is_builtin() { "Eingebauter Bildschirm".into() } else { format!("Bildschirm {}", index + 1) },
                left: bounds.origin.x as i32,
                top: bounds.origin.y as i32,
                width: display.pixels_wide() as u32,
                height: display.pixels_high() as u32,
                primary: display.is_main(),
            }
        })
        .collect())
}

/// The display's id and its bounds in points, for input.
pub(crate) fn display_geometry(index: u8) -> Option<(u32, core_graphics::geometry::CGRect)> {
    let id = *CGDisplay::active_displays().ok()?.get(index as usize)?;
    Some((id, CGDisplay::new(id).bounds()))
}

/// The newest picture, held until taken.
struct Latest {
    surface: Option<usize>,
    stopped: bool,
    /// The capturer is gone: late frames are not held any more.
    closed: bool,
}

/// Keeps a surface: retained, and marked in use so the stream does not draw
/// the next picture into it while it is read (as OBS does).
///
/// SAFETY: `surface` must be valid.
unsafe fn hold(surface: IOSurfaceRef) {
    CFRetain(surface as *const c_void);
    IOSurfaceIncrementUseCount(surface);
}

/// Undoes [`hold`].
///
/// SAFETY: `surface` must have been passed to `hold`.
unsafe fn let_go(surface: IOSurfaceRef) {
    IOSurfaceDecrementUseCount(surface);
    CFRelease(surface as *const c_void);
}

struct Shared {
    latest: Mutex<Latest>,
    fresh: Condvar,
}

pub struct Capturer {
    display: Display,
    stream: CGDisplayStreamRef,
    queue: *mut c_void,
    shared: Arc<Shared>,
    /// The handler must live as long as the stream.
    _handler: RcBlock<dyn Fn(i32, u64, IOSurfaceRef, *const c_void)>,
    /// The pointer is part of the picture; the viewer's own one is hidden once.
    pointer_sent: bool,
}

// SAFETY: the stream and queue are only used through the thread-safe CG and
// dispatch calls in `new` and `drop`; the handler touches only `Shared`.
unsafe impl Send for Capturer {}

impl Capturer {
    pub fn new(index: u8) -> Result<Self> {
        if !screen_permission(true) {
            bail!("Bitte CTXRemote in den Systemeinstellungen unter Datenschutz & Sicherheit → Bildschirmaufnahme erlauben");
        }
        let display = displays()?.into_iter().find(|d| d.index == index).context("Bildschirm nicht gefunden")?;
        let (id, _) = display_geometry(index).context("Bildschirm nicht gefunden")?;
        let shared = Arc::new(Shared { latest: Mutex::new(Latest { surface: None, stopped: false, closed: false }), fresh: Condvar::new() });
        let handler = {
            let shared = shared.clone();
            RcBlock::new(move |status: i32, _time: u64, surface: IOSurfaceRef, _update: *const c_void| {
                let mut latest = shared.latest.lock().unwrap_or_else(|e| e.into_inner());
                match status {
                    FRAME_COMPLETE if !surface.is_null() && !latest.closed => {
                        // SAFETY: the surface is valid during the call; held
                        // here and let go when replaced or taken.
                        unsafe { hold(surface) };
                        if let Some(old) = latest.surface.replace(surface as usize) {
                            unsafe { let_go(old as IOSurfaceRef) };
                        }
                        shared.fresh.notify_one();
                    }
                    FRAME_STOPPED => {
                        latest.stopped = true;
                        shared.fresh.notify_one();
                    }
                    // Idle or blank frames: nothing new to show.
                    _ => {}
                }
            })
        };
        let properties = CFDictionary::from_CFType_pairs(&[
            // SAFETY: framework constants, valid for the process lifetime.
            (unsafe { CFString::wrap_under_get_rule(kCGDisplayStreamShowCursor) }, CFBoolean::true_value().as_CFType()),
            (unsafe { CFString::wrap_under_get_rule(kCGDisplayStreamMinimumFrameTime) }, CFNumber::from(1.0f64 / 30.0).as_CFType()),
            (unsafe { CFString::wrap_under_get_rule(kCGDisplayStreamQueueDepth) }, CFNumber::from(3i32).as_CFType()),
        ]);
        // SAFETY: arguments are valid; the queue and stream are released in `drop`.
        let (queue, stream) = unsafe {
            let queue = dispatch_queue_create(c"ctxremote.capture".as_ptr(), std::ptr::null());
            let stream = CGDisplayStreamCreateWithDispatchQueue(
                id,
                display.width as usize,
                display.height as usize,
                PIXEL_FORMAT_BGRA,
                properties.as_concrete_TypeRef() as *const c_void,
                queue,
                &handler,
            );
            (queue, stream)
        };
        if stream.is_null() {
            // SAFETY: created above, not used elsewhere.
            unsafe { dispatch_release(queue) };
            bail!("Bildschirmaufnahme nicht startbar");
        }
        // SAFETY: a stream created above.
        if unsafe { CGDisplayStreamStart(stream) } != 0 {
            unsafe {
                CFRelease(stream as *const c_void);
                dispatch_release(queue);
            }
            bail!("Bildschirmaufnahme nicht startbar");
        }
        Ok(Self { display, stream, queue, shared, _handler: handler, pointer_sent: false })
    }

    pub fn display(&self) -> &Display {
        &self.display
    }

    /// Waits up to `timeout_ms` for a new picture and hands it to `consume`
    /// (BGRA, row pitch, width, height). `false` if nothing changed.
    pub fn next_frame(&mut self, timeout_ms: u32, consume: impl FnOnce(&[u8], usize, u32, u32)) -> Result<bool> {
        let surface = {
            let latest = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
            let (mut latest, _) = self
                .shared
                .fresh
                .wait_timeout_while(latest, Duration::from_millis(timeout_ms.into()), |l| l.surface.is_none() && !l.stopped)
                .unwrap_or_else(|e| e.into_inner());
            if latest.stopped {
                bail!("Bildschirmaufnahme beendet");
            }
            match latest.surface.take() {
                Some(surface) => surface as IOSurfaceRef,
                None => return Ok(false),
            }
        };
        // SAFETY: a surface retained by the handler; locked for reading while
        // its memory is used, then released.
        unsafe {
            let locked = IOSurfaceLock(surface, LOCK_READ_ONLY, std::ptr::null_mut()) == 0;
            if locked {
                let (width, height) = (IOSurfaceGetWidth(surface), IOSurfaceGetHeight(surface));
                let pitch = IOSurfaceGetBytesPerRow(surface);
                let base = IOSurfaceGetBaseAddress(surface) as *const u8;
                if !base.is_null() && width > 0 && height > 0 {
                    consume(std::slice::from_raw_parts(base, pitch * height), pitch, width as u32, height as u32);
                }
                IOSurfaceUnlock(surface, LOCK_READ_ONLY, std::ptr::null_mut());
            }
            let_go(surface);
            Ok(locked)
        }
    }

    /// The pointer is drawn into the picture, so the viewer's own pointer is
    /// hidden: one fully transparent shape, once.
    pub fn take_pointer(&mut self) -> Option<CursorShape> {
        if self.pointer_sent {
            return None;
        }
        self.pointer_sent = true;
        Some(CursorShape { width: 1, height: 1, hot_x: 0, hot_y: 0, rgba: vec![0; 4] })
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        // Stopping is asynchronous: a frame arriving after this is not held.
        let mut latest = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
        latest.closed = true;
        if let Some(surface) = latest.surface.take() {
            // SAFETY: held by the handler.
            unsafe { let_go(surface as IOSurfaceRef) };
        }
        drop(latest);
        // SAFETY: stream and queue from `new`.
        unsafe {
            CGDisplayStreamStop(self.stream);
            CFRelease(self.stream as *const c_void);
            dispatch_release(self.queue);
        }
    }
}
