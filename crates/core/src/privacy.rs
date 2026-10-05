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
//! The window and hooks belong to a thread of this process; they end with
//! [`PrivacyMode`], and with the process if it dies.

use anyhow::Result;

pub struct PrivacyMode {
    #[cfg(windows)]
    inner: imp::Running,
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
}

#[cfg(windows)]
impl Drop for PrivacyMode {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use anyhow::{bail, Context, Result};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateFontW, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject, SelectObject,
        SetBkMode, SetTextColor, BLACK_BRUSH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL,
        HBRUSH, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
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
    }

    impl Running {
        pub fn start() -> Result<Self> {
            let (ready, started) = mpsc::channel::<Result<u32, String>>();
            let thread = std::thread::Builder::new()
                .name("ctxremote-privacy".into())
                .spawn(move || run(ready))
                .context("Thread nicht startbar")?;
            match started.recv() {
                Ok(Ok(thread_id)) => Ok(Self { thread: Some(thread), thread_id }),
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
            SetTimer(Some(window), 1, RAISE_MS, None);
            let _ = ready.send(Ok(GetCurrentThreadId()));
            tracing::info!("Privatsphäre-Modus an");

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = KillTimer(Some(window), 1);
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
