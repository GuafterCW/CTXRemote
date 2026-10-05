//! Drawing over the host's screen: the viewer's lines on a transparent,
//! click-through window over the shown display. Unlike privacy mode it is
//! not hidden from capture, so the viewer sees the lines in the picture.

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
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, LineTo, MoveToEx,
        SelectObject, PAINTSTRUCT, PS_SOLID,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW, GetWindowLongPtrW,
        PostMessageW, PostQuitMessage, PostThreadMessageW, RegisterClassW, SetLayeredWindowAttributes,
        SetWindowLongPtrW, SetWindowPos, TranslateMessage, GWLP_USERDATA, HWND_TOPMOST, LWA_COLORKEY, MSG,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, WM_APP, WM_DESTROY, WM_PAINT, WM_QUIT, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{Display, Line};

    /// Painted as see-through; no line color equals it (see `add`).
    const KEY: COLORREF = COLORREF(0x00FF_00FF);
    const REDRAW: u32 = WM_APP + 1;

    type Lines = Arc<Mutex<Vec<Line>>>;

    pub struct Running {
        thread: Option<JoinHandle<()>>,
        thread_id: u32,
        /// The window, as a number: `HWND` is not `Send`.
        window: isize,
        lines: Lines,
    }

    impl Running {
        pub fn start(display: &Display) -> Result<Self> {
            let lines: Lines = Arc::default();
            let (ready, started) = mpsc::channel::<Result<(u32, isize), String>>();
            let (rect, shared) = ((display.left, display.top, display.width as i32, display.height as i32), lines.clone());
            let thread = std::thread::Builder::new()
                .name("ctxremote-draw".into())
                .spawn(move || run(rect, shared, ready))
                .context("Thread nicht startbar")?;
            match started.recv() {
                Ok(Ok((thread_id, window))) => Ok(Self { thread: Some(thread), thread_id, window, lines }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    bail!(e)
                }
                Err(_) => bail!("Zeichenebene nicht startbar"),
            }
        }

        pub fn add(&self, mut line: Line) {
            // The see-through color would punch holes instead of drawing.
            if line.color == 0x00FF_00FF {
                line.color = 0x00FE_00FF;
            }
            self.lines.lock().unwrap().push(line);
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

    fn run(rect: (i32, i32, i32, i32), lines: Lines, ready: mpsc::Sender<Result<(u32, isize), String>>) {
        crate::desktop::follow_input();
        // SAFETY: Win32 calls on this thread's own window and queue; the
        // pointer stored in the window lives until the window is destroyed.
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
            let window = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                class,
                w!("CTXRemote"),
                WS_POPUP,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
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
            let _ = SetLayeredWindowAttributes(window, KEY, 255, LWA_COLORKEY);
            let shared = Box::into_raw(Box::new(lines));
            SetWindowLongPtrW(window, GWLP_USERDATA, shared as isize);
            let _ = SetWindowPos(window, Some(HWND_TOPMOST), rect.0, rect.1, rect.2, rect.3, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = ready.send(Ok((GetCurrentThreadId(), window.0 as isize)));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            let _ = DestroyWindow(window);
            drop(Box::from_raw(shared));
        }
    }

    unsafe extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            REDRAW => {
                let _ = InvalidateRect(Some(window), None, false);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(window, &mut ps);
                let mut rect = RECT::default();
                let _ = GetClientRect(window, &mut rect);
                let key = CreateSolidBrush(KEY);
                FillRect(dc, &rect, key);
                let _ = DeleteObject(key.into());
                let shared = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Lines;
                if let Some(lines) = shared.as_ref() {
                    for line in lines.lock().unwrap().iter() {
                        // Windows wants 0x00BBGGRR.
                        let (r, g, b) = ((line.color >> 16) & 0xff, (line.color >> 8) & 0xff, line.color & 0xff);
                        let pen = CreatePen(PS_SOLID, line.width as i32, COLORREF(b << 16 | g << 8 | r));
                        let old = SelectObject(dc, pen.into());
                        let mut points = line.points.iter();
                        if let Some(&(x, y)) = points.next() {
                            let _ = MoveToEx(dc, x, y, None);
                            // A single point still shows as a dot.
                            let _ = LineTo(dc, x + 1, y);
                            let _ = MoveToEx(dc, x, y, None);
                            for &(x, y) in points {
                                let _ = LineTo(dc, x, y);
                            }
                        }
                        SelectObject(dc, old);
                        let _ = DeleteObject(pen.into());
                    }
                }
                let _ = EndPaint(window, &ps);
                LRESULT(0)
            }
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
}
