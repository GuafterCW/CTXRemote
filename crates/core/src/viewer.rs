//! The controlling side of a session.

use std::time::Duration;

use anyhow::{bail, Result};
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::ClientMsg;
use ctxremote_proto::secure::viewer_handshake;
use ctxremote_proto::session::{HostInfo, HostMsg, VideoFrame, ViewerMsg};
use ctxremote_proto::DeviceId;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::net;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The host may first ask its user (see `Host::require_approval`).
const WELCOME_TIMEOUT: Duration = Duration::from_secs(90);

pub enum ViewerEvent {
    Video(VideoFrame),
    Clipboard(String),
    /// The session ended; carries the reason if it was not the viewer's choice.
    Closed(Option<String>),
}

pub struct ViewerSession {
    pub host: HostInfo,
    outbox: mpsc::UnboundedSender<ViewerMsg>,
}

impl ViewerSession {
    /// Connects through `server` and authenticates with `password`.
    /// `on_event` runs on the network task and must not block.
    pub async fn connect(
        server: &str,
        own_id: Option<DeviceId>,
        target: DeviceId,
        password: &str,
        on_event: impl Fn(ViewerEvent) + Send + 'static,
    ) -> Result<Self> {
        let (mut t, _) = net::dial(server).await?;
        framing::send(&mut t, &ClientMsg::Connect { target }).await?;
        net::await_ready(&mut t, CONNECT_TIMEOUT).await?;

        let (mut tx, mut rx) = viewer_handshake(t, target, password).await?;
        let name = format!("{} ({})", whoami::username(), whoami::devicename());
        tx.send(&ViewerMsg::Hello { name, device: own_id }).await?;
        let host = match timeout(WELCOME_TIMEOUT, rx.recv::<HostMsg>()).await?? {
            Some(HostMsg::Welcome(info)) => info,
            Some(HostMsg::Bye(reason)) => bail!(reason),
            _ => bail!("Gegenstelle hat die Sitzung nicht eröffnet"),
        };

        let (outbox, mut outgoing) = mpsc::unbounded_channel::<ViewerMsg>();
        tokio::spawn(async move {
            while let Some(msg) = outgoing.recv().await {
                let bye = matches!(msg, ViewerMsg::Bye);
                if tx.send(&msg).await.is_err() || bye {
                    break;
                }
            }
            tx.close().await;
        });

        tokio::spawn(async move {
            let reason = loop {
                match rx.recv::<HostMsg>().await {
                    Ok(Some(HostMsg::Video(frame))) => on_event(ViewerEvent::Video(frame)),
                    Ok(Some(HostMsg::Clipboard(text))) => on_event(ViewerEvent::Clipboard(text)),
                    Ok(Some(HostMsg::Bye(reason))) => break Some(reason),
                    Ok(Some(HostMsg::Welcome(_))) => {}
                    Ok(None) => break None,
                    Err(e) => break Some(format!("Verbindung unterbrochen: {e}")),
                }
            };
            on_event(ViewerEvent::Closed(reason));
        });

        Ok(Self { host, outbox })
    }

    pub fn send(&self, msg: ViewerMsg) {
        let _ = self.outbox.send(msg);
    }

    /// A handle for sending from other threads, e.g. the clipboard watcher.
    pub fn sender(&self) -> mpsc::UnboundedSender<ViewerMsg> {
        self.outbox.clone()
    }

    pub fn close(&self) {
        self.send(ViewerMsg::Bye);
    }
}

impl Drop for ViewerSession {
    fn drop(&mut self) {
        self.close();
    }
}
