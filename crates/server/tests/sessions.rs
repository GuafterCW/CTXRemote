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
