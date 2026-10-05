//! Privacy mode: the host's monitors show a black screen and its local mouse
//! and keyboard do nothing, while the viewer still sees and controls the
//! desktop.
//!
//! On Windows a black, click-through window covers all monitors. It is
//! excluded from capture (`WDA_EXCLUDEFROMCAPTURE`, Windows 10 2004 and
//! later), so the viewer sees what lies beneath. Low-level hooks swallow
//! every input that was not injected, i.e. everything but the viewer's.
//! Ctrl+Alt+Del cannot be blocked, which leaves the person at the computer
//! a way out (and the secure desktop it opens is not covered).
//!
//! The pointer is drawn above every window, the cover included, so the
//! system pointers are swapped for an invisible one while the mode runs.
//! Screen capture then sees that invisible pointer, too; the viewer gets the
//! original shapes from copies taken before the swap (see
//! [`PrivacyMode::pointer`]). Leaving the mode loads the user's pointers again,
//! and so does the next agent if one died meanwhile ([`restore_pointers`]).
//!
//! The window and hooks belong to a thread of this process; they end with
//! [`PrivacyMode`], and with the process if it dies.

use anyhow::Result;
use ctxremote_proto::session::CursorShape;

pub struct PrivacyMode {
    #[cfg(windows)]
    inner: imp::Running,
}

/// Brings back the user's pointers if an earlier agent hid them and ended
/// without doing so (a crash, a killed process).
pub fn restore_pointers() {
    #[cfg(windows)]
    imp::restore_if_left();
}

/// Whether a pointer shape from capture is the invisible stand-in.
pub fn is_blank(shape: &CursorShape) -> bool {
    shape.rgba.chunks_exact(4).all(|px| px[3] == 0)
}

impl PrivacyMode {
    pub fn start() -> Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self { inner: imp::Running::start()? })
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("Den Privatsphäre-Modus gibt es nur unter Windows")
        }
    }

    /// The real shape of the pointer, if it changed since the last call and
    /// is one of the swapped system pointers (other pointers stay visible to
    /// capture). Polled while the mode runs.
    pub fn pointer(&mut self) -> Option<CursorShape> {
        #[cfg(windows)]
        {
            self.inner.pointer()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

#[cfg(windows)]
impl Drop for PrivacyMode {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::sync::{mpsc, Mutex};
    use std::thread::JoinHandle;

    use ctxremote_proto::session::CursorShape;

    use anyhow::{bail, Context, Result};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateFontW, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetDIBits, GetObjectW,
        GetStockObject, ReleaseDC, SelectObject, SetBkMode, SetTextColor, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, BLACK_BRUSH, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, HBITMAP,
        HBRUSH, PAINTSTRUCT, RGBQUAD, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, CopyIcon, CreateCursor, CreateWindowExW, DestroyIcon, GetCursorInfo, GetIconInfo,
        LoadCursorW, SetSystemCursor, SystemParametersInfoW, CURSORINFO, CURSOR_SHOWING, HICON,
        ICONINFO, IDC_APPSTARTING, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_HELP, IDC_IBEAM, IDC_NO, IDC_SIZEALL,
        IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, IDC_UPARROW, IDC_WAIT, OCR_APPSTARTING, OCR_CROSS,
        OCR_HAND, OCR_HELP, OCR_IBEAM, OCR_NO, OCR_NORMAL, OCR_SIZEALL, OCR_SIZENESW, OCR_SIZENS, OCR_SIZENWSE,
        OCR_SIZEWE, OCR_UP, OCR_WAIT, SPI_SETCURSORS, SYSTEM_CURSOR_ID, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, GetSystemMetrics, KillTimer, PostQuitMessage, PostThreadMessageW, RegisterClassW,
        SetLayeredWindowAttributes, SetTimer, SetWindowDisplayAffinity, SetWindowPos, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, HHOOK, HWND_TOPMOST, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
        LLMHF_INJECTED, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, WDA_EXCLUDEFROMCAPTURE,
        WH_KEYBOARD_LL, WH_MOUSE_LL, WM_DESTROY, WM_PAINT, WM_QUIT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    const TEXT: PCWSTR = w!("Dieser Computer wird gerade aus der Ferne gewartet. Strg+Alt+Entf gibt Maus und Tastatur frei.");
    /// Other topmost windows may come up; the cover goes back on top this often.
    const RAISE_MS: u32 = 500;

    pub struct Running {
        thread: Option<JoinHandle<()>>,
        thread_id: u32,
        /// The pointer handle last reported by [`Running::pointer`].
        last_pointer: Option<isize>,
    }

    /// The swapped system pointers: their shared handle and a copy of the
    /// original image. Shared handles are the same in every process.
    static ORIGINALS: Mutex<Vec<(isize, isize)>> = Mutex::new(Vec::new());

    const SYSTEM_POINTERS: [(PCWSTR, SYSTEM_CURSOR_ID); 14] = [
        (IDC_ARROW, OCR_NORMAL),
        (IDC_IBEAM, OCR_IBEAM),
        (IDC_WAIT, OCR_WAIT),
        (IDC_CROSS, OCR_CROSS),
        (IDC_UPARROW, OCR_UP),
        (IDC_SIZENWSE, OCR_SIZENWSE),
        (IDC_SIZENESW, OCR_SIZENESW),
        (IDC_SIZEWE, OCR_SIZEWE),
        (IDC_SIZENS, OCR_SIZENS),
        (IDC_SIZEALL, OCR_SIZEALL),
        (IDC_NO, OCR_NO),
        (IDC_HAND, OCR_HAND),
        (IDC_APPSTARTING, OCR_APPSTARTING),
        (IDC_HELP, OCR_HELP),
    ];

    /// Exists while the pointers are swapped, so a later agent can undo it.
    fn marker() -> PathBuf {
        std::env::temp_dir().join("ctxremote-pointers-hidden")
    }

    pub fn restore_if_left() {
        if marker().exists() {
            tracing::info!("Mauszeiger eines früheren Privatsphäre-Modus wiederhergestellt");
            restore_pointers();
        }
    }

    /// Swaps every system pointer for an invisible one, keeping copies.
    unsafe fn hide_pointers() {
        let _ = std::fs::write(marker(), b"");
        let mut originals = ORIGINALS.lock().unwrap_or_else(|e| e.into_inner());
        for (name, id) in SYSTEM_POINTERS {
            let Ok(shared) = LoadCursorW(None, name) else { continue };
            let Ok(copy) = CopyIcon(HICON(shared.0)) else { continue };
            // 32×32, AND plane all ones and XOR plane all zeros: nothing drawn.
            let and = [0xFFu8; 32 * 32 / 8];
            let xor = [0u8; 32 * 32 / 8];
            let Ok(blank) = CreateCursor(None, 0, 0, 32, 32, and.as_ptr().cast(), xor.as_ptr().cast()) else {
                let _ = DestroyIcon(copy);
                continue;
            };
            // Takes ownership of `blank`.
            if SetSystemCursor(blank, id).is_ok() {
                originals.push((shared.0 as isize, copy.0 as isize));
            } else {
                let _ = DestroyIcon(HICON(blank.0));
                let _ = DestroyIcon(copy);
            }
        }
    }

    /// Loads the user's pointer scheme again and drops the copies.
    fn restore_pointers() {
        // SAFETY: reloads the pointers from the user's settings; no pointers passed.
        unsafe {
            let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        let mut originals = ORIGINALS.lock().unwrap_or_else(|e| e.into_inner());
        for (_, copy) in originals.drain(..) {
            // SAFETY: copies made by `hide_pointers`, destroyed once.
            unsafe {
                let _ = DestroyIcon(HICON(copy as *mut _));
            }
        }
        let _ = std::fs::remove_file(marker());
    }

    /// The pointer's original image as straight RGBA.
    unsafe fn shape_of(icon: HICON) -> Option<CursorShape> {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let result = (|| {
            let mut mask = BITMAP::default();
            if GetObjectW(info.hbmMask.into(), size_of::<BITMAP>() as i32, Some((&mut mask as *mut BITMAP).cast())) == 0 {
                return None;
            }
            let width = mask.bmWidth as u32;
            let mask_height = mask.bmHeight as u32;
            if width == 0 || width > 256 || mask_height == 0 || mask_height > 512 {
                return None;
            }
            let mask_pitch = (width as usize).div_ceil(32) * 4;
            let mask_bits = bits(info.hbmMask, width, mask_height, 1, mask_pitch)?;
            let (height, rgba) = if info.hbmColor.is_invalid() {
                // Monochrome: AND and XOR planes stacked in the mask.
                let height = mask_height / 2;
                let rgba = crate::capture::pointer_rgba(
                    crate::capture::PointerFormat::Monochrome,
                    width,
                    height,
                    mask_pitch,
                    &mask_bits,
                )?;
                (height, rgba)
            } else {
                let height = mask_height;
                let color = bits(info.hbmColor, width, height, 32, width as usize * 4)?;
                let mut rgba = crate::capture::pointer_rgba(
                    crate::capture::PointerFormat::Color,
                    width,
                    height,
                    width as usize * 4,
                    &color,
                )?;
                // Old colour pointers have no alpha; the AND mask says what is drawn.
                if rgba.chunks_exact(4).all(|px| px[3] == 0) {
                    for y in 0..height as usize {
                        for x in 0..width as usize {
                            let transparent = mask_bits[y * mask_pitch + x / 8] & (0x80 >> (x % 8)) != 0;
                            let px = &mut rgba[(y * width as usize + x) * 4..][..4];
                            px[3] = match (transparent, px[..3] == [0, 0, 0]) {
                                (false, _) => 255,
                                (true, true) => 0,
                                // Inverts the screen; see `pointer_rgba`.
                                (true, false) => {
                                    px[..3].copy_from_slice(&[0, 0, 0]);
                                    160
                                }
                            };
                        }
                    }
                }
                (height, rgba)
            };
            Some(CursorShape { width, height, hot_x: info.xHotspot, hot_y: info.yHotspot, rgba })
        })();
        let _ = DeleteObject(info.hbmMask.into());
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        result
    }

    /// A bitmap's rows, top-down, at `bit_count` bits per pixel.
    unsafe fn bits(bitmap: HBITMAP, width: u32, height: u32, bit_count: u16, pitch: usize) -> Option<Vec<u8>> {
        // Room for the two-colour palette of a 1-bit request.
        #[repr(C)]
        struct Info {
            header: BITMAPINFOHEADER,
            colors: [RGBQUAD; 2],
        }
        let mut info = Info {
            header: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: bit_count,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            colors: [RGBQUAD::default(); 2],
        };
        let mut out = vec![0u8; pitch * height as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height,
            Some(out.as_mut_ptr().cast()),
            (&mut info as *mut Info).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        (lines == height as i32).then_some(out)
    }

    impl Running {
        pub fn start() -> Result<Self> {
            let (ready, started) = mpsc::channel::<Result<u32, String>>();
            let thread = std::thread::Builder::new()
                .name("ctxremote-privacy".into())
                .spawn(move || run(ready))
                .context("Thread nicht startbar")?;
            match started.recv() {
                Ok(Ok(thread_id)) => Ok(Self { thread: Some(thread), thread_id, last_pointer: None }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    bail!(e)
                }
                Err(_) => bail!("Privatsphäre-Modus nicht startbar"),
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

        pub fn pointer(&mut self) -> Option<CursorShape> {
            let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
            // SAFETY: fills a struct of ours with the declared size.
            unsafe { GetCursorInfo(&mut info) }.ok()?;
            if info.flags.0 & CURSOR_SHOWING.0 == 0 {
                return None;
            }
            let handle = info.hCursor.0 as isize;
            if self.last_pointer == Some(handle) {
                return None;
            }
            self.last_pointer = Some(handle);
            let copy = ORIGINALS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(s, _)| *s == handle)?.1;
            // SAFETY: a copy owned by ORIGINALS, which lives until `restore_pointers`
            // on the privacy thread; that runs only after `stop`, which needs `&mut self`.
            unsafe { shape_of(HICON(copy as *mut _)) }
        }
    }

    fn run(ready: mpsc::Sender<Result<u32, String>>) {
        // The cover and hooks must be on the desktop the person sees.
        crate::desktop::follow_input();
        // SAFETY: Win32 calls on this thread's own window, hooks and queue;
        // everything created here is released before the thread ends.
        unsafe {
            let window = match create_window() {
                Ok(window) => window,
                Err(e) => {
                    let _ = ready.send(Err(format!("{e:#}")));
                    return;
                }
            };
            // Without the exclusion the viewer would see black, too.
            if SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE).is_err() {
                let _ = DestroyWindow(window);
                let _ = ready.send(Err(
                    "Der Privatsphäre-Modus braucht Windows 10 (Version 2004) oder neuer".into(),
                ));
                return;
            }
            let module = GetModuleHandleW(None).unwrap_or_default();
            let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(module.into()), 0);
            let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(module.into()), 0);
            let (keyboard, mouse) = match (keyboard, mouse) {
                (Ok(k), Ok(m)) => (k, m),
                (k, m) => {
                    unhook(k.ok());
                    unhook(m.ok());
                    let _ = DestroyWindow(window);
                    let _ = ready.send(Err("Maus und Tastatur am Gerät nicht sperrbar".into()));
                    return;
                }
            };
            cover(window);
            hide_pointers();
            SetTimer(Some(window), 1, RAISE_MS, None);
            let _ = ready.send(Ok(GetCurrentThreadId()));
            tracing::info!("Privatsphäre-Modus an");

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = KillTimer(Some(window), 1);
            restore_pointers();
            unhook(Some(keyboard));
            unhook(Some(mouse));
            let _ = DestroyWindow(window);
            tracing::info!("Privatsphäre-Modus aus");
        }
    }

    unsafe fn unhook(hook: Option<HHOOK>) {
        if let Some(hook) = hook {
            let _ = UnhookWindowsHookEx(hook);
        }
    }

    unsafe fn create_window() -> Result<HWND> {
        let instance = GetModuleHandleW(None).context("Modul unbekannt")?;
        let class = w!("CTXRemotePrivacy");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        // Fails harmlessly when the class exists from an earlier start.
        RegisterClassW(&wc);
        // Layered and transparent: the viewer's injected clicks reach the
        // windows underneath; the person's own input is stopped by the hooks.
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            class,
            w!("CTXRemote"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("Abdeckung nicht anlegbar")?;
        SetLayeredWindowAttributes(window, COLORREF(0), 255, LWA_ALPHA).context("Abdeckung nicht einstellbar")?;
        Ok(window)
    }

    /// Covers all monitors and goes on top.
    unsafe fn cover(window: HWND) {
        let _ = SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    unsafe extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER => {
                cover(window);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(window, &mut ps);
                let mut rect = RECT::default();
                let _ = GetClientRect(window, &mut rect);
                FillRect(dc, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
                let font = CreateFontW(
                    28, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, Default::default(), Default::default(),
                    Default::default(), Default::default(), 0, w!("Segoe UI"),
                );
                let old = SelectObject(dc, font.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00B0_B0B0));
                let mut text: Vec<u16> = TEXT.as_wide().to_vec();
                DrawTextW(dc, &mut text, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                SelectObject(dc, old);
                let _ = DeleteObject(font.into());
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

    /// Lets through only what was injected, i.e. the viewer's keys.
    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if event.flags.0 & LLKHF_INJECTED.0 == 0 {
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    /// Lets through only what was injected, i.e. the viewer's mouse.
    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            if event.flags & LLMHF_INJECTED == 0 {
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}
