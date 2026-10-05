//! A session that reaches its host only through the UDP path (hole punching
//! via the server's reflector). Its own test binary, because it switches the
//! viewer's TCP attempt off through the environment.

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn session_moves_to_punched_connection() {
    std::env::set_var("CTXREMOTE_DIRECT", "udp");
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, true).await;
    let (session, events) = connect(&addr, &host, id).await;
    assert!(session.features.has(Features::PUNCH));

    tokio::task::spawn_blocking(move || {
        // TCP is off, so any direct route is the punched one.
        wait_for(&events, |e| match e {
            ViewerEvent::Direct(addr) => Some(addr),
            ViewerEvent::Closed(reason) => panic!("Sitzung beendet: {reason:?}"),
            _ => None,
        })
        .expect("Sitzung wechselt auf den UDP-Weg");
        for n in 0..50 {
            echo(&session, &events, &format!("durch NAT {n}"));
        }
        // A keyframe-sized message still gets through.
        let big = "x".repeat(2 * 1024 * 1024);
        echo(&session, &events, &big);
    })
    .await
    .unwrap();
}

