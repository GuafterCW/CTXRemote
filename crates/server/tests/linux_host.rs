//! A whole session with the Linux host on a real X server: the viewer gets a
//! picture and the pointer shape, and its mouse moves the X pointer. Needs a
//! server in `DISPLAY` (`xvfb-run -s "-noreset -screen 0 1280x800x24" …`);
//! without one the test passes without doing anything.
#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use common::*;
use ctxremote_core::viewer::ViewerEvent;
use ctxremote_proto::session::{InputEvent, ViewerMsg};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;

#[tokio::test(flavor = "multi_thread")]
async fn viewer_sees_and_moves_the_linux_screen() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("kein X-Server (DISPLAY), Test übersprungen");
        return;
    }
    let (_server, addr) = start_server().await;
    let (host, id) = start_real_host(&addr).await;
    let (session, events) = connect(&addr, &host, id).await;

    let (mut video, mut cursor) = (false, false);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !(video && cursor) && std::time::Instant::now() < deadline {
        match events.recv_timeout(Duration::from_secs(1)) {
            Ok(ViewerEvent::Video(frame)) => {
                assert!(!frame.data.is_empty());
                video = true;
            }
            Ok(ViewerEvent::Cursor(shape)) => {
                assert!(shape.width > 0 && shape.rgba.len() == (shape.width * shape.height * 4) as usize);
                cursor = true;
            }
            Ok(ViewerEvent::Closed(reason)) => panic!("Sitzung beendet: {reason:?}"),
            _ => {}
        }
    }
    assert!(video, "Bild angekommen");
    assert!(cursor, "Zeigerform angekommen");

    session.send(ViewerMsg::Input(InputEvent::MouseMove { x: 200, y: 150 }));
    let (conn, n) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[n].root;
    let mut at = (0, 0);
    for _ in 0..50 {
        let p = conn.query_pointer(root).unwrap().reply().unwrap();
        at = (p.root_x, p.root_y);
        if at == (200, 150) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(at, (200, 150), "Zeiger bewegt");
}
