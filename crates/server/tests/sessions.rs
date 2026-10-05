//! Whole sessions through a real server: host, viewer and the switch to a
//! direct connection. The screen side is a stand-in that echoes clipboard
//! texts, so this runs on any platform.

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn session_moves_to_direct_connection() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, true).await;
    let (session, events) = connect(&addr, &host, id).await;
    assert_eq!(session.features, Features::CURRENT);

    let route = tokio::task::spawn_blocking(move || {
        let route = wait_for(&events, |e| match e {
            ViewerEvent::Direct(addr) => Some(addr),
            _ => None,
        });
        (route, events)
    })
    .await
    .unwrap();
    let (route, events) = route;
    let route = route.expect("Sitzung wechselt auf die Direktverbindung");
    // Loopback is never offered; the route is one of this machine's interface addresses.
    assert!(!route.starts_with("127.0.0.1:"), "{route}");

    let session = tokio::task::spawn_blocking(move || {
        for n in 0..50 {
            echo(&session, &events, &format!("direkt {n}"));
        }
        session
    })
    .await
    .unwrap();
    drop(session);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_listener_the_session_stays_on_the_relay() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let (session, events) = connect(&addr, &host, id).await;
    tokio::task::spawn_blocking(move || {
        echo(&session, &events, "über den Server");
        assert!(wait_for(&events, |e| matches!(e, ViewerEvent::Direct(_)).then_some(())).is_none());
        echo(&session, &events, "immer noch");
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_goes_both_ways() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let mut host_events = host.events();
    let (session, events) = connect(&addr, &host, id).await;

    let number = loop {
        match tokio::time::timeout(Duration::from_secs(5), host_events.recv()).await.unwrap().unwrap() {
            ctxremote_core::host::HostEvent::SessionStarted { session, chat, .. } => {
                assert!(chat, "a current viewer can chat");
                break session;
            }
            _ => continue,
        }
    };
    session.send(ViewerMsg::Chat("  Hallo vom Viewer  ".into()));
    let text = loop {
        match tokio::time::timeout(Duration::from_secs(5), host_events.recv()).await.unwrap().unwrap() {
            ctxremote_core::host::HostEvent::Chat { session, text } if session == number => break text,
            _ => continue,
        }
    };
    assert_eq!(text, "Hallo vom Viewer");

    host.send_chat(number, "Antwort vom Host").unwrap();
    assert!(host.send_chat(number, "   ").is_err());
    tokio::task::spawn_blocking(move || {
        let got = wait_for(&events, |e| match e {
            ViewerEvent::Chat(t) => Some(t),
            _ => None,
        });
        assert_eq!(got.as_deref(), Some("Antwort vom Host"));
        drop(session);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn public_alias_reaches_the_device() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;

    assert_eq!(host.set_public_alias(Some(" Test-Rechner ".into())).await.unwrap().as_deref(), Some("test-rechner"));
    assert_eq!(host.public_alias().as_deref(), Some("test-rechner"));
    assert_eq!(ctxremote_core::alias::resolve(&addr, "TEST-rechner").await.unwrap(), Some(id));
    assert_eq!(ctxremote_core::alias::resolve(&addr, "gibt-es-nicht").await.unwrap(), None);

    // Another device cannot take it.
    let (other, _) = start_host(&addr, false).await;
    let err = other.set_public_alias(Some("test-rechner".into())).await.unwrap_err();
    assert!(err.contains("vergeben"), "{err}");
    assert!(other.set_public_alias(Some("12345".into())).await.is_err());

    // Connecting through the resolved alias works like through the ID.
    let resolved = ctxremote_core::alias::resolve(&addr, "test-rechner").await.unwrap().unwrap();
    let (session, events) = connect(&addr, &host, resolved).await;
    tokio::task::spawn_blocking(move || echo(&session, &events, "per Alias")).await.unwrap();

    // Dropping it frees the name for others.
    assert_eq!(host.set_public_alias(None).await.unwrap(), None);
    assert_eq!(other.set_public_alias(Some("test-rechner".into())).await.unwrap().as_deref(), Some("test-rechner"));
}

#[tokio::test(flavor = "multi_thread")]
async fn helper_profile_reaches_the_host() {
    use ctxremote_core::host::HostEvent;
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let mut events = host.events();
    let profile = HelperProfile {
        name: "Philipp".into(),
        company: "Ecker IT".into(),
        message: "Ich schaue mir den Drucker an".into(),
        logo: vec![],
    };
    let (session, viewer_events) = connect_as(&addr, &host, id, Some(profile)).await;

    let started = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(HostEvent::SessionStarted { profile, .. }) = events.recv().await {
                return profile;
            }
        }
    })
    .await
    .unwrap()
    .expect("Profil kommt an");
    assert_eq!(started.label(), "Philipp (Ecker IT)");
    assert_eq!(started.message, "Ich schaue mir den Drucker an");
    assert_eq!(host.session_profiles().len(), 1);
    tokio::task::spawn_blocking(move || echo(&session, &viewer_events, "mit Profil")).await.unwrap();
}

/// Clients from before encryption still get answers, for the transition.
#[tokio::test(flavor = "multi_thread")]
async fn unencrypted_clients_still_work() {
    use ctxremote_core::proto::framing;
    use ctxremote_core::proto::rendezvous::{ClientMsg, ServerMsg};
    let (_server, addr) = start_server().await;
    let mut t = framing::transport(tokio::net::TcpStream::connect(&addr).await.unwrap());
    assert!(matches!(framing::recv::<ServerMsg>(&mut t).await.unwrap(), ServerMsg::Challenge { .. }));
    framing::send(&mut t, &ClientMsg::UpdateCheck { platform: "test".into() }).await.unwrap();
    assert!(matches!(framing::recv::<ServerMsg>(&mut t).await.unwrap(), ServerMsg::Update(None)));
}

#[tokio::test(flavor = "multi_thread")]
async fn sound_reaches_the_viewer_once_asked_for() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let (session, events) = connect(&addr, &host, id).await;
    assert!(session.set_audio(true), "der Host kann Ton");
    let packet = tokio::task::spawn_blocking(move || {
        wait_for(&events, |e| match e {
            ViewerEvent::Audio(data) => Some(data),
            _ => None,
        })
    })
    .await
    .unwrap();
    assert_eq!(packet, Some(vec![0xf8, 1, 2, 3]));
    session.set_audio(false);
}

fn describe(event: &Option<ViewerEvent>) -> String {
    match event {
        None => "nichts".into(),
        Some(ViewerEvent::Rights(r)) => format!("Rights({})", r.0),
        Some(ViewerEvent::Privacy { on, error }) => format!("Privacy {{ on: {on}, error: {error:?} }}"),
        Some(ViewerEvent::Clipboard(t)) => format!("Clipboard({t})"),
        Some(ViewerEvent::ClipboardImage(_)) => "ClipboardImage".into(),
        Some(ViewerEvent::Audio(_)) => "Audio".into(),
        Some(ViewerEvent::Closed(r)) => format!("Closed({r:?})"),
        Some(_) => "anderes Ereignis".into(),
    }
}

/// The next event that is not video or a pointer, so tests see what matters.
fn next_event(events: &std_mpsc::Receiver<ViewerEvent>, wait: Duration) -> Option<ViewerEvent> {
    let deadline = std::time::Instant::now() + wait;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match events.recv_timeout(left) {
            Ok(ViewerEvent::Video(_) | ViewerEvent::Cursor(_)) => continue,
            Ok(event) => return Some(event),
            Err(_) => return None,
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn host_rights_are_enforced_and_can_change() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let (session, events) = connect(&addr, &host, id).await;
    let number = host.sessions()[0].0;

    // Without the right, file requests get an answer instead of waiting.
    host.set_rights(number, Permissions::ATTENDED.with(Permissions::FILES, false)).unwrap();
    let listed = tokio::time::timeout(Duration::from_secs(10), session.files().list(String::new())).await;
    let error = listed.expect("Antwort kam").expect_err("Dateien sind gesperrt");
    assert!(format!("{error:#}").contains("nicht erlaubt"), "{error:#}");
    host.set_rights(number, Permissions::ATTENDED).unwrap();

    tokio::task::spawn_blocking(move || {
        // One-time password: someone sits at the computer, so no privacy mode.
        // The changes above arrive in order: start, without files, back again.
        let mut seen = Vec::new();
        wait_for(&events, |e| match e {
            ViewerEvent::Rights(r) => {
                seen.push(r);
                (seen.len() == 3).then_some(())
            }
            _ => None,
        });
        let without_files = Permissions::ATTENDED.with(Permissions::FILES, false);
        assert_eq!(seen, vec![Permissions::ATTENDED, without_files, Permissions::ATTENDED]);
        assert!(session.set_privacy(true));
        match next_event(&events, Duration::from_secs(10)) {
            Some(ViewerEvent::Privacy { on: false, error: Some(_) }) => {}
            other => panic!("Privatsphäre-Modus hätte abgelehnt werden müssen: {}", describe(&other)),
        }
        echo(&session, &events, "erlaubt");
        session.send(ViewerMsg::ClipboardImage(vec![0x89, b'P', b'N', b'G']));
        match next_event(&events, Duration::from_secs(10)) {
            Some(ViewerEvent::ClipboardImage(png)) => assert_eq!(png, vec![0x89, b'P', b'N', b'G']),
            other => panic!("Bild nicht angekommen: {}", describe(&other)),
        }

        // View only: clipboard and sound no longer reach the screen side.
        host.set_rights(number, Permissions::VIEW_ONLY).unwrap();
        match next_event(&events, Duration::from_secs(10)) {
            Some(ViewerEvent::Rights(r)) => assert_eq!(r, Permissions::VIEW_ONLY),
            other => panic!("Rechte nicht gemeldet: {}", describe(&other)),
        }
        session.send(ViewerMsg::Clipboard("gesperrt".into()));
        session.send(ViewerMsg::ClipboardImage(vec![1, 2, 3]));
        session.set_audio(true);
        let event = next_event(&events, Duration::from_secs(1));
        assert!(event.is_none(), "Nichts darf ankommen, kam aber: {}", describe(&event));

        // Everything, then privacy mode on; taking that right away ends it.
        host.set_rights(number, Permissions::ALL).unwrap();
        assert!(matches!(next_event(&events, Duration::from_secs(10)), Some(ViewerEvent::Rights(Permissions::ALL))));
        session.set_privacy(true);
        assert!(matches!(next_event(&events, Duration::from_secs(10)), Some(ViewerEvent::Privacy { on: true, error: None })));
        assert_eq!(host.session_rights(), vec![(number, Permissions::ALL, true)]);
        host.set_rights(number, Permissions::ATTENDED).unwrap();
        let mut saw_off = false;
        for _ in 0..2 {
            match next_event(&events, Duration::from_secs(10)) {
                Some(ViewerEvent::Rights(r)) => assert_eq!(r, Permissions::ATTENDED),
                Some(ViewerEvent::Privacy { on: false, .. }) => saw_off = true,
                other => panic!("unerwartet: {}", describe(&other)),
            }
        }
        assert!(saw_off, "Privatsphäre-Modus muss enden");
        assert_eq!(host.session_rights(), vec![(number, Permissions::ATTENDED, false)]);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn permanent_password_needs_the_authenticator_code() {
    use ctxremote_core::{totp, viewer::CodeNeeded};
    let (_server, addr) = start_server().await;
    let secret = totp::new_secret();
    let (host, id) = start_host_with(&addr, |config| {
        config.permanent_password = Some("dauerhaft-123".into());
        config.code_secret = Some(secret.clone());
    })
    .await;
    let attempt = |code: Option<String>| {
        let addr = addr.clone();
        async move { ViewerSession::connect(&addr, None, id, "dauerhaft-123", None, None, code.as_deref(), |_| {}).await }
    };

    let Err(e) = attempt(None).await else { panic!("ohne Code darf es nicht gehen") };
    assert!(e.downcast_ref::<CodeNeeded>().is_some(), "{e:#}");

    // A wrong code (one that is not valid now or a step around).
    let valid: Vec<String> = [-30i64, 0, 30].iter().filter_map(|d| totp::code_at(&secret, (totp::now() as i64 + d) as u64)).collect();
    let wrong = (0..1_000_000).map(|n| format!("{n:06}")).find(|c| !valid.contains(c)).unwrap();
    let Err(e) = attempt(Some(wrong)).await else { panic!("falscher Code darf nicht gehen") };
    assert!(format!("{e:#}").contains("falsch"), "{e:#}");

    let code = totp::code_at(&secret, totp::now()).unwrap();
    let session = attempt(Some(code.clone())).await.expect("richtiger Code");
    session.close();
    // The same code a second time could be someone who saw it.
    let Err(e) = attempt(Some(code)).await else { panic!("derselbe Code zweimal") };
    assert!(format!("{e:#}").contains("schon benutzt"), "{e:#}");

    // The one-time password needs no code: someone at the computer handed it out.
    let session = ViewerSession::connect(&addr, None, id, &host.password(), None, None, None, |_| {}).await;
    assert!(session.is_ok(), "Einmalpasswort ohne Code");
}

#[tokio::test(flavor = "multi_thread")]
async fn system_info_reaches_the_viewer() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let (session, events) = connect(&addr, &host, id).await;
    assert!(session.request_system_info());
    let info = tokio::task::spawn_blocking(move || {
        wait_for(&events, |e| match e {
            ViewerEvent::SystemInfo(info) => Some(info),
            _ => None,
        })
    })
    .await
    .unwrap()
    .expect("Systeminfo kam an");
    assert!(info.memory_total > 0);
    assert!(!info.hostname.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn host_sees_that_the_viewer_records() {
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let (session, events) = connect(&addr, &host, id).await;
    let number = host.sessions()[0].0;
    session.set_recording(true);
    // A clipboard echo afterwards shows the host has read the message.
    tokio::task::spawn_blocking(move || {
        echo(&session, &events, "danach");
        session
    })
    .await
    .unwrap();
    assert_eq!(host.recording_sessions(), vec![number]);
}

#[tokio::test(flavor = "multi_thread")]
async fn port_tunnel_needs_its_right_and_carries_data() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = echo.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let (_server, addr) = start_server().await;
    let (host, id) = start_host(&addr, false).await;
    let (session, _events) = connect(&addr, &host, id).await;
    let number = host.sessions()[0].0;
    let port = session.tunnels().open(0, &target).await.unwrap();

    // One-time password: no way into the host's network.
    let mut refused = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(15), refused.read(&mut buf)).await.unwrap();
    assert!(matches!(n, Ok(0) | Err(_)), "ohne Recht muss die Verbindung enden");

    host.set_rights(number, Permissions::ALL).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
    let (mut r, mut w) = socket.split();
    let send = async {
        w.write_all(&payload).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let (_, got) = tokio::time::timeout(Duration::from_secs(30), async { tokio::join!(send, r.read_to_end(&mut back)) })
        .await
        .expect("Tunnel hängt");
    got.unwrap();
    assert!(back == payload, "{} von {} Bytes zurück", back.len(), payload.len());
}
