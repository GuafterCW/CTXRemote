//! A Linux host without a screen (a server without X11): the session still
//! starts, without displays and without a picture, and the terminal and the
//! processes work. Its own test binary, because it removes `DISPLAY` for the
//! whole process.
#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use common::*;
use ctxremote_core::host::HostEvent;
use ctxremote_core::viewer::ViewerEvent;
use ctxremote_proto::session::{Permissions, ProcessMsg, TerminalMsg};

#[tokio::test(flavor = "multi_thread")]
async fn a_host_without_a_screen_offers_terminal_and_processes() {
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
    let (_server, addr) = start_server().await;
    let (host, id) = start_real_host(&addr).await;
    let mut host_events = host.events();
    let (session, events) = connect(&addr, &host, id).await;
    assert!(session.host.displays.is_empty(), "{:?}", session.host.displays);
    let number = host.sessions()[0].0;

    host.set_rights(number, Permissions::ALL).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.terminal(TerminalMsg::Open { id: 1, cols: 80, rows: 24 });
    let opened = wait_for(&events, |e| match e {
        ViewerEvent::Terminal(TerminalMsg::Opened { id: 1, result }) => Some(result),
        ViewerEvent::Closed(reason) => panic!("Sitzung beendet: {reason:?}"),
        _ => None,
    });
    opened.expect("keine Antwort").expect("Terminal nicht gestartet");
    session.terminal(TerminalMsg::Data { id: 1, data: b"echo remocto-$((40+2))\n".to_vec() });
    let mut seen = String::new();
    let found = wait_for(&events, |e| match e {
        ViewerEvent::Terminal(TerminalMsg::Data { id: 1, data }) => {
            seen.push_str(&String::from_utf8_lossy(&data));
            seen.contains("remocto-42").then_some(())
        }
        ViewerEvent::Video(_) => panic!("Bild von einem Gerät ohne Bildschirm"),
        _ => None,
    });
    assert!(found.is_some(), "nur {seen:?}");

    // The person here (and a window opened later) learns it is the terminal alone.
    let only = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(HostEvent::Terminal { open: true, only, .. }) = host_events.recv().await {
                return only;
            }
        }
    })
    .await
    .expect("kein Terminal-Ereignis");
    assert!(only);
    assert_eq!(host.terminal_sessions(), vec![(number, true)]);

    let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    session.processes(ProcessMsg::List);
    let listing = wait_for(&events, |e| match e {
        ViewerEvent::Processes(ProcessMsg::Listing(listing)) => Some(listing),
        _ => None,
    });
    let listing = listing.expect("keine Liste").expect("Liste verweigert");
    let target = listing.processes.iter().find(|p| p.pid == child.id()).expect("Kindprozess fehlt").clone();
    session.processes(ProcessMsg::Kill { pid: target.pid, name: target.name.clone() });
    let done = wait_for(&events, |e| match e {
        ViewerEvent::Processes(ProcessMsg::Done { what, result }) => Some((what, result)),
        _ => None,
    });
    let (what, result) = done.expect("keine Antwort");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(what, format!("Prozess {} beendet", target.name));
    assert!(!child.wait().unwrap().success());

    // Both are in this computer's history.
    let actions = &host.history()[0].actions;
    assert_eq!(actions, &["Terminal geöffnet".to_string(), what], "{actions:?}");

    session.terminal(TerminalMsg::Data { id: 1, data: b"exit\n".to_vec() });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(host.terminal_sessions().is_empty());
}
