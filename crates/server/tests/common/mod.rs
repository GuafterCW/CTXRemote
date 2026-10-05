//! Helpers shared by the tests with a real server.
#![allow(dead_code)]

pub use std::process::{Child, Command};
pub use std::sync::mpsc as std_mpsc;
pub use std::sync::{Arc, RwLock};
pub use std::time::Duration;

pub use ctxremote_core::config::Config;
pub use ctxremote_core::host::{Host, Presence, ScreenChannels, ScreenSource};
pub use ctxremote_core::proto::session::{Features, HelperProfile, HostInfo, HostMsg, ViewerMsg};
pub use ctxremote_core::proto::DeviceId;
pub use ctxremote_core::viewer::{ViewerEvent, ViewerSession};
pub use futures::future::BoxFuture;
pub use tokio::sync::mpsc;

pub struct Echo;

impl ScreenSource for Echo {
    fn open(&self) -> BoxFuture<'static, anyhow::Result<ScreenChannels>> {
        let (to_agent, mut inbox) = mpsc::channel::<ViewerMsg>(64);
        let (outbox, from_agent) = mpsc::channel::<HostMsg>(8);
        tokio::spawn(async move {
            let info = HostInfo {
                hostname: "echo".into(),
                username: "test".into(),
                os: "test".into(),
                displays: vec![],
                active_display: 0,
            };
            if outbox.send(HostMsg::Welcome(info)).await.is_err() {
                return;
            }
            while let Some(msg) = inbox.recv().await {
                if let ViewerMsg::Clipboard(text) = msg {
                    if outbox.send(HostMsg::Clipboard(text)).await.is_err() {
                        return;
                    }
                }
            }
        });
        Box::pin(async move { Ok((to_agent, from_agent)) })
    }
}

pub struct Server(pub Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

pub async fn start_server() -> (Server, String) {
    let port = free_port();
    let data = std::env::temp_dir().join(format!("ctxremote-test-server-{port}"));
    let child = Command::new(env!("CARGO_BIN_EXE_ctxremote-server"))
        .args(["--listen", &format!("127.0.0.1:{port}"), "--data"])
        .arg(&data)
        .spawn()
        .unwrap();
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (Server(child), addr)
}

pub async fn start_host(server: &str, direct: bool) -> (Host, DeviceId) {
    let config_path = std::env::temp_dir().join(format!("ctxremote-test-host-{}.json", free_port()));
    std::env::set_var("CTXREMOTE_CONFIG", &config_path);
    let config = Config {
        server: server.to_string(),
        direct,
        direct_port: free_port(),
        ..Config::default()
    };
    let host = Host::start_with(Arc::new(RwLock::new(config)), Arc::new(Echo));
    let mut presence = host.presence();
    let id = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Presence::Online { id } = presence.borrow_and_update().clone() {
                return id.parse().unwrap();
            }
            presence.changed().await.unwrap();
        }
    })
    .await
    .expect("host online");
    // Lets the listener for direct connections come up.
    tokio::time::sleep(Duration::from_millis(200)).await;
    (host, id)
}

/// Waits for the next event that `pick` accepts.
pub fn wait_for<T>(events: &std_mpsc::Receiver<ViewerEvent>, mut pick: impl FnMut(ViewerEvent) -> Option<T>) -> Option<T> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match events.recv_timeout(left) {
            Ok(event) => {
                if let Some(found) = pick(event) {
                    return Some(found);
                }
            }
            Err(_) => return None,
        }
    }
    None
}

pub fn echo(session: &ViewerSession, events: &std_mpsc::Receiver<ViewerEvent>, text: &str) {
    session.send(ViewerMsg::Clipboard(text.into()));
    let got = wait_for(events, |e| match e {
        ViewerEvent::Clipboard(t) => Some(t),
        ViewerEvent::Closed(reason) => panic!("Sitzung beendet: {reason:?}"),
        _ => None,
    });
    assert_eq!(got.as_deref(), Some(text));
}

pub async fn connect(server: &str, host: &Host, id: DeviceId) -> (ViewerSession, std_mpsc::Receiver<ViewerEvent>) {
    connect_as(server, host, id, None).await
}

pub async fn connect_as(
    server: &str,
    host: &Host,
    id: DeviceId,
    profile: Option<HelperProfile>,
) -> (ViewerSession, std_mpsc::Receiver<ViewerEvent>) {
    let (tx, rx) = std_mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let session = ViewerSession::connect(server, None, id, &host.password(), profile, move |event| {
        let _ = tx.lock().unwrap().send(event);
    })
    .await
    .unwrap();
    (session, rx)
}

