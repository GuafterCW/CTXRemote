//! Drawing over the host's screen: the viewer's lines on a transparent,
//! click-through window over the shown display. Unlike privacy mode it is
//! not hidden from capture, so the viewer sees the lines in the picture.
//!
//! The lines are rasterised here with smooth edges ([`Canvas`]) and shown
//! with per-pixel alpha; GDI pens on a colour-keyed window looked jagged.

use anyhow::Result;
use ctxremote_proto::session::DrawMsg;

use crate::capture::Display;

/// One line, in the display's own pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub color: u32,
    pub width: u8,
    pub points: Vec<(i32, i32)>,
}

/// Scales a stroke's 0..=65535 points to a display of `width` × `height`.
pub fn to_line(msg: &DrawMsg, width: u32, height: u32) -> Option<Line> {
    let DrawMsg::Stroke { color, width: pen, points } = msg else { return None };
    if points.is_empty() {
        return None;
    }
    let scale = |v: u16, size: u32| ((v as u64 * size.saturating_sub(1) as u64) / 65535) as i32;
    Some(Line {
        color: *color & 0x00ff_ffff,
        width: (*pen).clamp(1, 24),
        points: points.iter().take(4096).map(|&(x, y)| (scale(x, width), scale(y, height))).collect(),
    })
}

/// Premultiplied BGRA pixels (`0xAARRGGBB`), as a 32-bit DIB holds them.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, pixels: vec![0; width * height] }
    }

    /// See [`stroke`].
    pub fn stroke(&mut self, line: &Line) -> Option<Dirty> {
        stroke(&mut self.pixels, self.width, self.height, line)
    }
}

/// The part of the pixels a stroke changed: left, top, right, bottom (exclusive).
pub type Dirty = (usize, usize, usize, usize);

/// Draws a line with round ends and joins and soft edges onto premultiplied
/// pixels (`width` × `height`, top-down). Where it meets its own colour, the
/// stronger coverage wins instead of adding up, so pieces of one stroke and
/// their shared ends blend seamlessly.
pub fn stroke(pixels: &mut [u32], width: usize, height: usize, line: &Line) -> Option<Dirty> {
    let Some(&first) = line.points.first() else { return None };
    // Half a pixel of soft edge on each side of the pen.
    let reach = line.width as f32 / 2.0 + 0.5;
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for &(x, y) in &line.points {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let grow = reach.ceil() as i64 + 1;
    let clip = |v: i64, max: usize| v.clamp(0, max as i64) as usize;
    let (left, right) = (clip(x0 as i64 - grow, width), clip(x1 as i64 + grow + 1, width));
    let (top, bottom) = (clip(y0 as i64 - grow, height), clip(y1 as i64 + grow + 1, height));
    if left >= right || top >= bottom || pixels.len() < width * height {
        return None;
    }
    let w = right - left;
    // Coverage of this line: per pixel, the largest of its segments'.
    let mut coverage = vec![0f32; w * (bottom - top)];
    let point = |p: (i32, i32)| (p.0 as f32, p.1 as f32);
    let segments: Vec<((f32, f32), (f32, f32))> = if line.points.len() == 1 {
        vec![(point(first), point(first))]
    } else {
        line.points.windows(2).map(|p| (point(p[0]), point(p[1]))).collect()
    };
    for ((ax, ay), (bx, by)) in segments {
        let sx0 = clip((ax.min(bx) - reach).floor() as i64, width).max(left);
        let sy0 = clip((ay.min(by) - reach).floor() as i64, height).max(top);
        let sx1 = clip((ax.max(bx) + reach).ceil() as i64 + 1, width).min(right);
        let sy1 = clip((ay.max(by) + reach).ceil() as i64 + 1, height).min(bottom);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        for y in sy0..sy1 {
            for x in sx0..sx1 {
                // Distance from the pixel to the segment.
                let (px, py) = (x as f32 - ax, y as f32 - ay);
                let t = if len2 > 0.0 { ((px * dx + py * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (ex, ey) = (px - t * dx, py - t * dy);
                let c = (reach - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);
                let cell = &mut coverage[(y - top) * w + (x - left)];
                if c > *cell {
                    *cell = c;
                }
            }
        }
    }
    let rgb = [(line.color >> 16) & 0xff, (line.color >> 8) & 0xff, line.color & 0xff];
    let premultiplied = |a: u32| a << 24 | (rgb[0] * a + 127) / 255 << 16 | (rgb[1] * a + 127) / 255 << 8 | (rgb[2] * a + 127) / 255;
    for y in top..bottom {
        for x in left..right {
            let c = coverage[(y - top) * w + (x - left)];
            if c <= 0.0 {
                continue;
            }
            let a = (c * 255.0).round() as u32;
            let px = &mut pixels[y * width + x];
            let below = *px >> 24;
            if below > 0 && *px == premultiplied(below) {
                // The same colour: the stronger coverage.
                *px = premultiplied(a.max(below));
                continue;
            }
            // Source over destination, both premultiplied.
            let over = |shift: u32, src: u32| {
                let dst = (*px >> shift) & 0xff;
                ((src * a + dst * (255 - a) + 127) / 255) << shift
            };
            *px = over(24, 255) | over(16, rgb[0]) | over(8, rgb[1]) | over(0, rgb[2]);
        }
    }
    Some((left, top, right, bottom))
}

pub struct Overlay {
    #[cfg(windows)]
    inner: imp::Running,
    #[cfg(not(windows))]
    _none: (),
}

impl Overlay {
    pub fn start(display: &Display) -> Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self { inner: imp::Running::start(display)? })
        }
        #[cfg(not(windows))]
        {
            let _ = display;
            anyhow::bail!("Zeichnen gibt es nur unter Windows")
        }
    }

    pub fn add(&self, line: Line) {
        #[cfg(windows)]
        self.inner.add(line);
        #[cfg(not(windows))]
        let _ = line;
    }
}

#[cfg(windows)]
impl Drop for Overlay {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::JoinHandle;

    use anyhow::{bail, Context, Result};
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage,
        PostThreadMessageW, RegisterClassW, SetWindowPos, TranslateMessage, UpdateLayeredWindowIndirect, HWND_TOPMOST,
        MSG, UPDATELAYEREDWINDOWINFO,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, ULW_ALPHA, WM_APP, WM_DESTROY, WM_QUIT, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{Display, Dirty, Line};

    const REDRAW: u32 = WM_APP + 1;

    /// Lines waiting for the window's thread to draw them.
    type Pending = Arc<Mutex<Vec<Line>>>;

    pub struct Running {
        thread: Option<JoinHandle<()>>,
        thread_id: u32,
        /// The window, as a number: `HWND` is not `Send`.
        window: isize,
        pending: Pending,
    }

    impl Running {
        pub fn start(display: &Display) -> Result<Self> {
            let pending: Pending = Arc::default();
            let (ready, started) = mpsc::channel::<Result<(u32, isize), String>>();
            let (rect, shared) = ((display.left, display.top, display.width as i32, display.height as i32), pending.clone());
            let thread = std::thread::Builder::new()
                .name("ctxremote-draw".into())
                .spawn(move || run(rect, shared, ready))
                .context("Thread nicht startbar")?;
            match started.recv() {
                Ok(Ok((thread_id, window))) => Ok(Self { thread: Some(thread), thread_id, window, pending }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    bail!(e)
                }
                Err(_) => bail!("Zeichenebene nicht startbar"),
            }
        }

        pub fn add(&self, line: Line) {
            self.pending.lock().unwrap().push(line);
            // SAFETY: a plain message to our own window; harmless if it is gone.
            unsafe {
                let _ = PostMessageW(Some(HWND(self.window as *mut _)), REDRAW, WPARAM(0), LPARAM(0));
            }
        }

        pub fn stop(&mut self) {
            if let Some(thread) = self.thread.take() {
                // SAFETY: plain message to a thread of ours with a message queue.
                unsafe {
                    let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                let _ = thread.join();
            }
        }
    }

    fn run(rect: (i32, i32, i32, i32), pending: Pending, ready: mpsc::Sender<Result<(u32, isize), String>>) {
        crate::desktop::follow_input();
        let (width, height) = (rect.2.max(1), rect.3.max(1));
        // SAFETY: Win32 calls on this thread's own window, DC and bitmap, all
        // released before the thread ends; `bits` points into the DIB section,
        // which lives until `bitmap` is deleted after the loop.
        unsafe {
            let instance = match GetModuleHandleW(None) {
                Ok(instance) => instance,
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            };
            let class = w!("CTXRemoteDraw");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            // Layered with per-pixel alpha, and transparent: clicks go through.
            let window = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                class,
                w!("CTXRemote"),
                WS_POPUP,
                rect.0,
                rect.1,
                width,
                height,
                None,
                None,
                Some(instance.into()),
                None,
            ) {
                Ok(window) => window,
                Err(e) => {
                    let _ = ready.send(Err(format!("Zeichenebene nicht anlegbar: {e}")));
                    return;
                }
            };

            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Top-down, like the canvas.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bits.is_null() => bitmap,
                _ => {
                    let _ = DeleteDC(memory);
                    let _ = DestroyWindow(window);
                    let _ = ready.send(Err("Zeichenfläche nicht anlegbar".into()));
                    return;
                }
            };
            let old = SelectObject(memory, bitmap.into());
            // The lines are drawn straight into the DIB section (zeroed: fully see-through).
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (width * height) as usize);
            let (origin, size, source) = (POINT { x: rect.0, y: rect.1 }, SIZE { cx: width, cy: height }, POINT::default());
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // Only the changed part goes to the screen.
            let show = |dirty: Dirty| {
                let dirty = RECT { left: dirty.0 as i32, top: dirty.1 as i32, right: dirty.2 as i32, bottom: dirty.3 as i32 };
                let info = UPDATELAYEREDWINDOWINFO {
                    cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                    hdcDst: Default::default(),
                    pptDst: &origin,
                    psize: &size,
                    hdcSrc: memory,
                    pptSrc: &source,
                    crKey: Default::default(),
                    pblend: &blend,
                    dwFlags: ULW_ALPHA,
                    prcDirty: &dirty,
                };
                let _ = UpdateLayeredWindowIndirect(window, &info);
            };
            show((0, 0, width as usize, height as usize));
            let _ = SetWindowPos(window, Some(HWND_TOPMOST), rect.0, rect.1, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = ready.send(Ok((GetCurrentThreadId(), window.0 as isize)));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == REDRAW {
                    let lines = std::mem::take(&mut *pending.lock().unwrap());
                    let mut changed: Option<Dirty> = None;
                    for line in &lines {
                        if let Some(d) = super::stroke(pixels, width as usize, height as usize, line) {
                            changed = Some(match changed {
                                Some(c) => (c.0.min(d.0), c.1.min(d.1), c.2.max(d.2), c.3.max(d.3)),
                                None => d,
                            });
                        }
                    }
                    if let Some(dirty) = changed {
                        show(dirty);
                    }
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            SelectObject(memory, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory);
            let _ = DestroyWindow(window);
        }
    }

    unsafe extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strokes_scale_to_the_display() {
        let msg = DrawMsg::Stroke { color: 0x12ff0000, width: 200, points: vec![(0, 0), (65535, 65535), (32768, 0)] };
        let line = to_line(&msg, 1920, 1080).unwrap();
        assert_eq!(line.color, 0xff0000);
        assert_eq!(line.width, 24);
        assert_eq!(line.points, vec![(0, 0), (1919, 1079), (959, 0)]);
        assert!(to_line(&DrawMsg::Clear, 10, 10).is_none());
        assert!(to_line(&DrawMsg::Stroke { color: 0, width: 3, points: vec![] }, 10, 10).is_none());
    }

    fn alpha(canvas: &Canvas, x: usize, y: usize) -> u32 {
        canvas.pixels[y * canvas.width + x] >> 24
    }

    #[test]
    fn strokes_are_smooth_and_premultiplied() {
        let mut canvas = Canvas::new(40, 20);
        canvas.stroke(&Line { color: 0xff0000, width: 4, points: vec![(5, 10), (30, 10)] });
        // Solid on the line, soft at its edge, nothing beyond.
        assert_eq!(canvas.pixels[10 * 40 + 15], 0xffff_0000, "deckend rot in der Mitte");
        let edge = alpha(&canvas, 15, 12);
        assert!(edge > 0 && edge < 255, "weicher Rand, Alpha {edge}");
        assert_eq!(alpha(&canvas, 15, 14), 0);
        // Premultiplied: no channel above alpha.
        for &px in &canvas.pixels {
            let a = px >> 24;
            assert!((px >> 16) & 0xff <= a && (px >> 8) & 0xff <= a && px & 0xff <= a);
        }
        // A round end beyond the last point; a single point is a dot.
        assert!(alpha(&canvas, 31, 10) > 0);
        canvas.stroke(&Line { color: 0x0000ff, width: 6, points: vec![(36, 4)] });
        assert_eq!(canvas.pixels[4 * 40 + 36], 0xff00_00ff);
        // Partly or wholly off the canvas: no panic.
        canvas.stroke(&Line { color: 0x00ff00, width: 24, points: vec![(-50, -50), (-30, -40)] });
        canvas.stroke(&Line { color: 0x00ff00, width: 8, points: vec![(39, 19), (400, 300)] });
        assert_eq!(alpha(&canvas, 39, 19), 255);
    }

    #[test]
    fn pieces_of_a_stroke_join_seamlessly() {
        let mut whole = Canvas::new(30, 20);
        whole.stroke(&Line { color: 0x30a46c, width: 4, points: vec![(3, 10), (15, 10), (27, 10)] });
        let mut pieces = Canvas::new(30, 20);
        pieces.stroke(&Line { color: 0x30a46c, width: 4, points: vec![(3, 10), (15, 10)] });
        pieces.stroke(&Line { color: 0x30a46c, width: 4, points: vec![(15, 10), (27, 10)] });
        assert_eq!(whole.pixels, pieces.pixels);
        // Another colour on top still covers it.
        pieces.stroke(&Line { color: 0xe5484d, width: 4, points: vec![(15, 2), (15, 18)] });
        assert_eq!(pieces.pixels[10 * 30 + 15], 0xffe5_484d);
    }

    #[test]
    fn a_line_over_itself_is_not_darker() {
        let mut once = Canvas::new(20, 20);
        once.stroke(&Line { color: 0x00ff00, width: 3, points: vec![(2, 10), (18, 10)] });
        let mut twice = Canvas::new(20, 20);
        twice.stroke(&Line { color: 0x00ff00, width: 3, points: vec![(2, 10), (18, 10), (2, 10)] });
        assert_eq!(once.pixels, twice.pixels);
    }
}
