//! The Linux host's capture and input against a real X server. Needs one in
//! `DISPLAY`, e.g. `xvfb-run -s "-noreset -screen 0 1280x800x24" cargo test -p ctxremote-core --test x11`;
//! without one the tests pass without doing anything.
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use ctxremote_core::capture::{displays, Capturer};
use ctxremote_core::input::Injector;
use ctxremote_proto::session::InputEvent;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// One test at a time: they share the X server's pointer and keyboard.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn x() -> Option<(RustConnection, Window)> {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("kein X-Server (DISPLAY), Test übersprungen");
        return None;
    }
    // Xvfb without -noreset restarts when the last client leaves: try again shortly.
    let mut attempt = 0;
    let (conn, n) = loop {
        match x11rb::connect(None) {
            Ok(c) => break c,
            Err(e) if attempt < 20 => {
                attempt += 1;
                eprintln!("X-Server noch nicht bereit: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("X-Server: {e}"),
        }
    };
    let root = conn.setup().roots[n].root;
    Some((conn, root))
}

/// Fills a rectangle of the root window with one colour (0xRRGGBB).
fn paint(conn: &RustConnection, root: Window, rect: Rectangle, rgb: u32) {
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, root, &CreateGCAux::new().foreground(rgb)).unwrap();
    conn.poly_fill_rectangle(root, gc, &[rect]).unwrap();
    conn.free_gc(gc).unwrap();
    conn.sync().unwrap();
}

#[test]
fn capture_sees_what_is_drawn() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((conn, root)) = x() else { return };
    let list = displays().expect("Bildschirme");
    assert!(!list.is_empty());
    assert!(list.iter().any(|d| d.primary));
    let display = list[0].clone();

    paint(&conn, root, Rectangle { x: 0, y: 0, width: display.width as u16, height: display.height as u16 }, 0x000000);
    let mut capturer = Capturer::new(0).expect("Aufnahme");
    let mut first = Vec::new();
    assert!(capturer
        .next_frame(10, |data, pitch, w, h| {
            assert_eq!((w, h), (display.width, display.height));
            assert_eq!(pitch, w as usize * 4);
            first = data.to_vec();
        })
        .unwrap());
    assert!(first.iter().all(|&b| b == 0 || b == 0xff), "schwarz");

    // Nothing drawn: no new picture, after waiting about one frame.
    let started = Instant::now();
    assert!(!capturer.next_frame(40, |_, _, _, _| panic!("unverändert")).unwrap());
    assert!(started.elapsed() >= Duration::from_millis(30), "wartet statt leer zu drehen");

    // A red square at (100, 50): BGRA in the picture.
    paint(&conn, root, Rectangle { x: 100, y: 50, width: 20, height: 20 }, 0xff0000);
    let mut pixel = [0u8; 4];
    assert!(capturer
        .next_frame(10, |data, pitch, _, _| pixel.copy_from_slice(&data[55 * pitch + 105 * 4..][..4]))
        .unwrap());
    assert_eq!(&pixel[..3], &[0x00, 0x00, 0xff], "rot als BGR");

    // The pointer shape is reported once.
    assert!(capturer.take_pointer().is_some());
    assert!(capturer.take_pointer().is_none());
}

#[test]
fn mouse_moves_and_keys_press() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((conn, root)) = x() else { return };
    let display = displays().unwrap().remove(0);
    let mut injector = Injector::new(&display);

    injector.apply(&InputEvent::MouseMove { x: 321, y: 123 });
    conn.sync().unwrap();
    let pointer = conn.query_pointer(root).unwrap().reply().unwrap();
    assert_eq!((pointer.root_x as i32, pointer.root_y as i32), (display.left + 321, display.top + 123));

    // Off-screen positions stay on the display.
    injector.apply(&InputEvent::MouseMove { x: 60000, y: 60000 });
    conn.sync().unwrap();
    let pointer = conn.query_pointer(root).unwrap().reply().unwrap();
    assert_eq!(pointer.root_x as i32, display.left + display.width as i32 - 1);

    injector.apply(&InputEvent::MouseButton { button: ctxremote_proto::session::MouseButton::Left, down: true });
    conn.sync().unwrap();
    let mask = conn.query_pointer(root).unwrap().reply().unwrap().mask;
    assert!(mask.contains(KeyButMask::BUTTON1), "linke Taste gedrückt");

    // Shift (X keycode 50) is down until released, also by "release all".
    injector.apply(&InputEvent::Key { code: "ShiftLeft".into(), down: true });
    conn.sync().unwrap();
    let keys = conn.query_keymap().unwrap().reply().unwrap().keys;
    assert!(keys[50 / 8] & (1 << (50 % 8)) != 0, "Umschalt gedrückt");

    injector.release_all();
    conn.sync().unwrap();
    let keys = conn.query_keymap().unwrap().reply().unwrap().keys;
    assert!(keys[50 / 8] & (1 << (50 % 8)) == 0, "Umschalt losgelassen");
    let mask = conn.query_pointer(root).unwrap().reply().unwrap().mask;
    assert!(!mask.contains(KeyButMask::BUTTON1), "Maustaste losgelassen");
}

#[test]
fn text_arrives_as_characters() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((conn, root)) = x() else { return };
    let display = displays().unwrap().remove(0);

    // A window with the keyboard focus that records its key presses.
    let window = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        200,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.sync().unwrap();
    conn.set_input_focus(InputFocus::POINTER_ROOT, window, x11rb::CURRENT_TIME).unwrap();
    conn.sync().unwrap();

    // Records the keysym each press means, read while the mapping holds it.
    let watcher = std::thread::spawn(move || {
        let setup = conn.setup().clone();
        let mut typed = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while typed.len() < 4 && Instant::now() < deadline {
            match conn.poll_for_event().unwrap() {
                Some(Event::KeyPress(e)) => {
                    let map = conn.get_keyboard_mapping(e.detail, 1).unwrap().reply().unwrap();
                    typed.push(map.keysyms[0]);
                }
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        let _ = setup;
        typed
    });
    std::thread::sleep(Duration::from_millis(50));
    let mut injector = Injector::new(&display);
    injector.apply(&InputEvent::Text("aÜ€\n".into()));
    let typed = watcher.join().unwrap();
    // a, Ü (Latin-1), € (Unicode keysym), Enter (its real key: Return).
    assert_eq!(typed, vec![0x61, 0xdc, 0x0100_20ac, 0xff0d]);
}
