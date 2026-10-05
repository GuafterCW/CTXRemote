//! CTXRemote-Setup.exe: a small window in the app's design that runs the
//! embedded NSIS installer silently and elevated, shows the progress and
//! offers to start the app. Automatic updates keep using the NSIS installer
//! directly (they run silently anyway). `CTXRemote-Setup.exe /S` installs
//! without a window, for administrators.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod text;
mod ui;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Icon, Theme, Window, WindowId};

use text::Fonts;
use ui::{Action, Existing, Hit, Phase, View};

enum Event {
    Finished(Result<(), install::Error>),
}

struct App {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    fonts: Fonts,
    view: View,
    cursor: PhysicalPosition<f64>,
    hits: Vec<(Hit, tiny_skia::Rect)>,
    proxy: EventLoopProxy<Event>,
}

fn existing() -> Existing {
    match install::installed_version() {
        None => Existing::None,
        Some(v) if install::newer(install::VERSION, &v) => Existing::Older(v),
        Some(v) if install::newer(&v, install::VERSION) => Existing::Newer(v),
        Some(_) => Existing::Same,
    }
}

/// The window and taskbar icon, drawn like the logo.
fn icon() -> Option<Icon> {
    let mut pixmap = Pixmap::new(64, 64)?;
    ui::logo(&mut pixmap, 0.0, 0.0, 64.0, 1.0);
    // tiny-skia stores premultiplied colours; the icon wants straight ones.
    let rgba = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    Icon::from_rgba(rgba.collect(), 64, 64).ok()
}

impl App {
    fn hit_at(&self, position: PhysicalPosition<f64>) -> Option<Hit> {
        let scale = self.window.as_ref()?.scale_factor();
        let (x, y) = ((position.x / scale) as f32, (position.y / scale) as f32);
        self.hits.iter().find(|(_, r)| x >= r.left() && x < r.right() && y >= r.top() && y < r.bottom()).map(|(h, _)| *h)
    }

    fn redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn activate(&mut self, hit: Hit, event_loop: &ActiveEventLoop) {
        if self.view.installing() {
            return;
        }
        match hit {
            Hit::Primary => match self.view.primary_action() {
                Some(Action::Install) => self.start(),
                Some(Action::Launch) => {
                    install::launch();
                    event_loop.exit();
                }
                None => {}
            },
            Hit::Secondary if self.view.secondary_launches() => {
                install::launch();
                event_loop.exit();
            }
            Hit::Secondary | Hit::Close => event_loop.exit(),
        }
    }

    fn start(&mut self) {
        self.view.phase = Phase::Installing { started: Instant::now() };
        self.view.notice = None;
        self.view.hover = None;
        let parent = self.window.as_ref().map_or(0, |w| hwnd(w));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(Event::Finished(install::run(parent)));
        });
        self.redraw();
    }

    fn paint(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else { return };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Some(mut pixmap) = Pixmap::new(size.width, size.height) else { return };
        self.hits = ui::render(&self.view, &mut self.fonts, &mut pixmap, window.scale_factor() as f32);
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        for (dst, src) in buffer.iter_mut().zip(pixmap.pixels()) {
            *dst = (src.red() as u32) << 16 | (src.green() as u32) << 8 | src.blue() as u32;
        }
        window.pre_present_notify();
        let _ = buffer.present();
    }
}

#[cfg(windows)]
fn hwnd(window: &Window) -> isize {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

#[cfg(not(windows))]
fn hwnd(_window: &Window) -> isize {
    0
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("CTXRemote Setup")
            .with_inner_size(LogicalSize::new(ui::WIDTH, ui::HEIGHT))
            .with_resizable(false)
            .with_decorations(false)
            .with_window_icon(icon())
            .with_visible(false);
        #[cfg(windows)]
        let attributes = {
            use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
            attributes
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round)
                .with_taskbar_icon(icon())
        };
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(_) => return event_loop.exit(),
        };
        // Centre on the screen the window opened on.
        if let Some(monitor) = window.current_monitor() {
            let (screen, at, size) = (monitor.size(), monitor.position(), window.outer_size());
            let x = at.x + (screen.width as i32 - size.width as i32) / 2;
            let y = at.y + (screen.height as i32 - size.height as i32) / 2;
            window.set_outer_position(PhysicalPosition::new(x, y));
        }
        self.view.dark = window.theme() == Some(Theme::Dark);
        let surface = softbuffer::Context::new(window.clone()).and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        match surface {
            Ok(surface) => self.surface = Some(surface),
            Err(_) => return event_loop.exit(),
        }
        self.window = Some(window.clone());
        self.paint();
        window.set_visible(true);
        window.focus_window();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Event) {
        let Event::Finished(result) = event;
        match result {
            Ok(()) => {
                self.view.phase = Phase::Done;
                self.view.existing = existing();
            }
            Err(install::Error::Declined) => {
                self.view.phase = Phase::Ready;
                self.view.notice = Some("Ohne Administratorrechte lässt sich nichts installieren.".into());
            }
            Err(install::Error::Failed(why)) => self.view.phase = Phase::Failed(why),
        }
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if !self.view.installing() {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::ThemeChanged(theme) => {
                self.view.dark = theme == Theme::Dark;
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                let hover = self.hit_at(position);
                if hover != self.view.hover {
                    self.view.hover = hover;
                    if let Some(window) = &self.window {
                        window.set_cursor(if hover.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
                    }
                    self.redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if self.view.hover.take().is_some() {
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let hit = self.hit_at(self.cursor);
                match state {
                    ElementState::Pressed => match hit {
                        Some(hit) => self.view.pressed = Some(hit),
                        // The window has no title bar: drag it anywhere else.
                        None => {
                            if let Some(window) = &self.window {
                                let _ = window.drag_window();
                            }
                        }
                    },
                    ElementState::Released => {
                        if let (Some(pressed), Some(hit)) = (self.view.pressed.take(), hit) {
                            if pressed == hit {
                                self.activate(hit, event_loop);
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key, state: ElementState::Pressed, .. }, .. } => {
                match logical_key {
                    Key::Named(NamedKey::Enter) if self.view.primary_action().is_some() => {
                        self.activate(Hit::Primary, event_loop)
                    }
                    Key::Named(NamedKey::Escape) => self.activate(Hit::Close, event_loop),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The progress bar moves while the installer runs.
        if self.view.installing() {
            self.redraw();
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(33)));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn view() -> View {
    View {
        phase: Phase::Ready,
        dark: false,
        existing: existing(),
        version: install::VERSION,
        preview: install::PAYLOAD.is_empty(),
        notice: None,
        hover: None,
        pressed: None,
    }
}

/// `--preview <ready|update|same|newer|installing|done|failed|declined> <out.png> [dark]`
/// renders one state at twice the size, to look at the design without Windows.
fn preview(args: &[String]) -> std::process::ExitCode {
    let (Some(state), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("--preview <Zustand> <Datei.png> [dark]");
        return std::process::ExitCode::FAILURE;
    };
    let Some(mut fonts) = Fonts::load() else {
        eprintln!("Keine Schrift gefunden");
        return std::process::ExitCode::FAILURE;
    };
    let mut view = view();
    view.dark = args.get(2).is_some_and(|a| a == "dark");
    view.existing = Existing::None;
    match state.as_str() {
        "update" => view.existing = Existing::Older("0.1.15".into()),
        "same" => view.existing = Existing::Same,
        "newer" => view.existing = Existing::Newer("0.2.3".into()),
        "installing" => view.phase = Phase::Installing { started: Instant::now() - Duration::from_secs(8) },
        "done" => view.phase = Phase::Done,
        "failed" => view.phase = Phase::Failed("Das Installationsprogramm meldet den Fehlercode 5.".into()),
        "declined" => view.notice = Some("Ohne Administratorrechte lässt sich nichts installieren.".into()),
        _ => {}
    }
    view.hover = args.get(3).and_then(|h| match h.as_str() {
        "primary" => Some(Hit::Primary),
        "secondary" => Some(Hit::Secondary),
        "close" => Some(Hit::Close),
        _ => None,
    });
    let scale = 2.0;
    let mut pixmap = Pixmap::new((ui::WIDTH * scale) as u32, (ui::HEIGHT * scale) as u32).unwrap();
    ui::render(&view, &mut fonts, &mut pixmap, scale);
    match pixmap.save_png(out) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--preview") {
        return preview(&args[1..]);
    }
    let silent = args.iter().any(|a| a.eq_ignore_ascii_case("/S"));
    let fonts = if silent { None } else { Fonts::load() };
    let Some(fonts) = fonts else {
        // Silent, or no font to draw with: install without a window.
        let ok = install::run(0).is_ok();
        if ok && !silent {
            install::launch();
        }
        return if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
    };
    let Ok(event_loop) = EventLoop::<Event>::with_user_event().build() else {
        return std::process::ExitCode::FAILURE;
    };
    let mut app = App {
        window: None,
        surface: None,
        fonts,
        view: view(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        hits: Vec::new(),
        proxy: event_loop.create_proxy(),
    };
    let _ = event_loop.run_app(&mut app);
    std::process::ExitCode::SUCCESS
}
