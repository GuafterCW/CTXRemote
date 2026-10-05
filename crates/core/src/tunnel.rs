//! Port tunnels: TCP connections carried inside a session (see
//! [`TunnelMsg`]). The viewer listens on a local port; for each connection
//! the host opens one to the target in its network, and both copy bytes.
//!
//! [`Streams`] is the part both sides share: it pumps one socket per
//! connection and keeps at most [`WINDOW`] bytes in flight each way, so a
//! fast transfer neither fills memory nor crowds out the picture.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::TunnelMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tracing::{debug, info};

/// Unacknowledged bytes per connection and direction.
pub const WINDOW: usize = 256 * 1024;
const CHUNK: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

struct Stream {
    /// Bytes from the peer, to write to the socket; `None` once the peer
    /// closed its side (the socket's write half then shuts down).
    to_socket: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// What the peer may still take: our reads wait on it.
    credit: Arc<Semaphore>,
    /// Reads from the socket; aborted when the connection is cut.
    reader: tokio::task::AbortHandle,
}

/// The connections of one session on one side.
pub struct Streams {
    out: mpsc::UnboundedSender<TunnelMsg>,
    open: Mutex<HashMap<u32, Stream>>,
}

impl Streams {
    /// `out` carries this side's messages to the peer.
    pub fn new(out: mpsc::UnboundedSender<TunnelMsg>) -> Arc<Self> {
        Arc::new(Self { out, open: Mutex::default() })
    }

    /// Pumps `socket` as connection `id` until both sides end.
    pub fn attach(self: &Arc<Self>, id: u32, socket: TcpStream) {
        let _ = socket.set_nodelay(true);
        let (mut reader, mut writer) = socket.into_split();
        let (to_socket, mut from_peer) = mpsc::unbounded_channel::<Vec<u8>>();
        let credit = Arc::new(Semaphore::new(WINDOW));
        let done = Arc::new(std::sync::atomic::AtomicU8::new(0));
        // Held until the entry is in place, so a quick end finds it.
        let mut open = self.open.lock().unwrap();

        // Socket → peer, waiting for credit. Its end is our `Close`.
        let this = self.clone();
        let (read_credit, read_done) = (credit.clone(), done.clone());
        let read = tokio::spawn(async move {
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let Ok(permit) = read_credit.acquire_many(n as u32).await else { break };
                permit.forget();
                if this.out.send(TunnelMsg::Data { id, data: buf[..n].to_vec() }).is_err() {
                    break;
                }
            }
            let _ = this.out.send(TunnelMsg::Close { id });
            this.half_done(id, &read_done);
        });

        // Peer → socket, acknowledging what was written; ends with the peer's `Close`.
        let this = self.clone();
        let write_done = done.clone();
        tokio::spawn(async move {
            while let Some(data) = from_peer.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
                let _ = this.out.send(TunnelMsg::Ack { id, bytes: data.len() as u32 });
            }
            let _ = writer.shutdown().await;
            this.half_done(id, &write_done);
        });

        // Halves done (reading, writing): at two the entry goes.
        drop(done);
        open.insert(id, Stream { to_socket: Some(to_socket), credit, reader: read.abort_handle() });
    }

    fn half_done(&self, id: u32, done: &std::sync::atomic::AtomicU8) {
        if done.fetch_add(1, Ordering::SeqCst) == 1 {
            self.open.lock().unwrap().remove(&id);
        }
    }

    /// Applies a message from the peer for an attached connection.
    pub fn handle(&self, msg: TunnelMsg) {
        let mut open = self.open.lock().unwrap();
        match msg {
            TunnelMsg::Data { id, data } => {
                if let Some(to_socket) = open.get(&id).and_then(|s| s.to_socket.as_ref()) {
                    let _ = to_socket.send(data);
                }
            }
            TunnelMsg::Ack { id, bytes } => {
                if let Some(stream) = open.get(&id) {
                    stream.credit.add_permits(bytes as usize);
                }
            }
            // The peer's side ended: what is queued still gets written, then
            // the socket's write half shuts. Our direction goes on.
            TunnelMsg::Close { id } => {
                if let Some(stream) = open.get_mut(&id) {
                    stream.to_socket = None;
                }
            }
            TunnelMsg::Open { .. } | TunnelMsg::Opened { .. } => {}
        }
    }

    /// Cuts every connection, e.g. when the right was taken away.
    pub fn close_all(&self) {
        let cut: Vec<(u32, Stream)> = self.open.lock().unwrap().drain().collect();
        for (id, stream) in cut {
            stream.reader.abort();
            stream.credit.close();
            let _ = self.out.send(TunnelMsg::Close { id });
        }
    }
}

/// The host's side: opens connections the viewer asks for.
pub struct TunnelHost {
    streams: Arc<Streams>,
}

impl TunnelHost {
    pub fn new(out: mpsc::UnboundedSender<TunnelMsg>) -> Self {
        Self { streams: Streams::new(out) }
    }

    pub fn handle(&self, msg: TunnelMsg) {
        match msg {
            TunnelMsg::Open { id, target } => {
                let streams = self.streams.clone();
                tokio::spawn(async move {
                    let result = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&target)).await {
                        Ok(Ok(socket)) => {
                            info!(%target, "Tunnel geöffnet");
                            streams.attach(id, socket);
                            Ok(())
                        }
                        Ok(Err(e)) => Err(format!("{target} nicht erreichbar: {e}")),
                        Err(_) => Err(format!("{target} antwortet nicht")),
                    };
                    let _ = streams.out.send(TunnelMsg::Opened { id, result });
                });
            }
            other => self.streams.handle(other),
        }
    }

    /// Refuses an `Open` the viewer may not make.
    pub fn refuse(&self, id: u32, reason: &str) {
        let _ = self.streams.out.send(TunnelMsg::Opened { id, result: Err(reason.to_string()) });
    }

    pub fn close_all(&self) {
        self.streams.close_all();
    }
}

/// A port this viewer listens on, and where the host connects for it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TunnelInfo {
    pub port: u16,
    pub target: String,
}

/// The viewer's side: local ports whose connections go through the host.
pub struct TunnelClient {
    streams: Arc<Streams>,
    next: AtomicU32,
    /// `Open`s waiting for the host's answer.
    waiting: Mutex<HashMap<u32, oneshot::Sender<Result<(), String>>>>,
    /// Port → (target, listener task).
    listeners: Mutex<HashMap<u16, (String, tokio::task::JoinHandle<()>)>>,
}

impl TunnelClient {
    pub fn new(out: mpsc::UnboundedSender<TunnelMsg>) -> Arc<Self> {
        Arc::new(Self {
            streams: Streams::new(out),
            next: AtomicU32::new(1),
            waiting: Mutex::default(),
            listeners: Mutex::default(),
        })
    }

    /// Listens on `127.0.0.1:port` (0: any free port) for connections to
    /// `target` behind the host; returns the port.
    pub async fn open(self: &Arc<Self>, port: u16, target: &str) -> Result<u16> {
        let target = target.trim().to_string();
        if !valid_target(&target) {
            bail!("Ziel bitte als Adresse:Port angeben, z. B. 192.168.1.10:3389");
        }
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| format!("Port {port} ist hier schon belegt"))?;
        let port = listener.local_addr()?.port();
        let this = self.clone();
        let to = target.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let this = this.clone();
                let to = to.clone();
                tokio::spawn(async move {
                    if let Err(e) = this.connect(socket, &to).await {
                        debug!("Tunnelverbindung abgelehnt: {e:#}");
                    }
                });
            }
        });
        self.listeners.lock().unwrap().insert(port, (target, task));
        Ok(port)
    }

    async fn connect(&self, socket: TcpStream, target: &str) -> Result<()> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, tx);
        if self.streams.out.send(TunnelMsg::Open { id, target: target.to_string() }).is_err() {
            bail!("Die Sitzung ist beendet");
        }
        match tokio::time::timeout(CONNECT_TIMEOUT + Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(()))) => {
                self.streams.attach(id, socket);
                Ok(())
            }
            Ok(Ok(Err(reason))) => bail!(reason),
            _ => {
                self.waiting.lock().unwrap().remove(&id);
                bail!("Keine Antwort vom Gerät")
            }
        }
    }

    pub fn handle(&self, msg: TunnelMsg) {
        match msg {
            TunnelMsg::Opened { id, result } => {
                if let Some(tx) = self.waiting.lock().unwrap().remove(&id) {
                    let _ = tx.send(result);
                }
            }
            other => self.streams.handle(other),
        }
    }

    /// Stops listening on `port`; its open connections end as well.
    pub fn close(&self, port: u16) {
        if let Some((_, task)) = self.listeners.lock().unwrap().remove(&port) {
            task.abort();
        }
        if self.listeners.lock().unwrap().is_empty() {
            self.streams.close_all();
        }
    }

    pub fn list(&self) -> Vec<TunnelInfo> {
        let mut list: Vec<TunnelInfo> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(port, (target, _))| TunnelInfo { port: *port, target: target.clone() })
            .collect();
        list.sort_by_key(|t| t.port);
        list
    }

    /// The session ended.
    pub fn closed(&self) {
        for (_, (_, task)) in self.listeners.lock().unwrap().drain() {
            task.abort();
        }
        for (_, tx) in self.waiting.lock().unwrap().drain() {
            let _ = tx.send(Err("Die Sitzung ist beendet".into()));
        }
        self.streams.close_all();
    }
}

/// `host:port` with a port that parses.
fn valid_target(target: &str) -> bool {
    target
        .rsplit_once(':')
        .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Viewer and host wired by channels; an echo server stands for the target.
    #[tokio::test]
    async fn bytes_flow_both_ways_with_flow_control() {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = echo.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });

        let (to_host, mut host_in) = mpsc::unbounded_channel();
        let (to_viewer, mut viewer_in) = mpsc::unbounded_channel();
        let client = TunnelClient::new(to_host);
        let host = Arc::new(TunnelHost::new(to_viewer));
        let h = host.clone();
        tokio::spawn(async move {
            while let Some(m) = host_in.recv().await {
                h.handle(m);
            }
        });
        let c = client.clone();
        tokio::spawn(async move {
            while let Some(m) = viewer_in.recv().await {
                c.handle(m);
            }
        });

        let port = client.open(0, &target).await.unwrap();
        assert_eq!(client.list().len(), 1);
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        // Several windows' worth, so the credit has to come back.
        let payload: Vec<u8> = (0..3 * WINDOW + 1234).map(|i| (i % 251) as u8).collect();
        let (mut r, mut w) = socket.split();
        let send = async {
            w.write_all(&payload).await.unwrap();
            w.shutdown().await.unwrap();
        };
        let mut back = Vec::new();
        let recv = r.read_to_end(&mut back);
        let (_, got) = tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(send, recv) }).await.unwrap();
        got.unwrap();
        assert_eq!(back.len(), payload.len());
        assert!(back == payload);

        assert!(client.open(0, "kein-port").await.is_err());
        client.close(port);
        assert!(client.list().is_empty());
    }

    #[tokio::test]
    async fn unreachable_target_closes_the_local_connection() {
        let (to_host, mut host_in) = mpsc::unbounded_channel();
        let (to_viewer, mut viewer_in) = mpsc::unbounded_channel();
        let client = TunnelClient::new(to_host);
        let host = Arc::new(TunnelHost::new(to_viewer));
        let h = host.clone();
        tokio::spawn(async move {
            while let Some(m) = host_in.recv().await {
                h.handle(m);
            }
        });
        let c = client.clone();
        tokio::spawn(async move {
            while let Some(m) = viewer_in.recv().await {
                c.handle(m);
            }
        });
        // A port nobody listens on.
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
        let port = client.open(0, &format!("127.0.0.1:{free}")).await.unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut buf = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(15), socket.read(&mut buf)).await.unwrap();
        assert!(matches!(n, Ok(0) | Err(_)), "die lokale Verbindung muss enden");
    }
}
