//! The viewer side: requests to the host's file browser and running transfers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use ctxremote_proto::session::{FileOp, FileReply, Listing, Transfer, ViewerMsg};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::{Incoming, Outgoing, WINDOW};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
/// An upload waiting this long for an acknowledgement gives up.
const ACK_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TransferEvent {
    Progress { id: u32, done: u64, total: u64 },
    /// `path` is where a download landed locally; uploads report `None`.
    Finished { id: u32, path: Option<String> },
    Failed { id: u32, message: String },
}

type EventSink = Arc<dyn Fn(TransferEvent) + Send + Sync>;

struct Upload {
    /// Bytes acknowledged by the host.
    acked: Mutex<u64>,
    wake: Condvar,
    stopped: AtomicBool,
    total: u64,
    reported: Mutex<Instant>,
}

impl Upload {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

/// Lives as long as the session; dropped transfers clean up after themselves.
pub struct FileClient {
    outbox: mpsc::UnboundedSender<ViewerMsg>,
    next: AtomicU32,
    pending: Mutex<HashMap<u32, oneshot::Sender<Result<FileReply, String>>>>,
    uploads: Mutex<HashMap<u32, Arc<Upload>>>,
    downloads: Mutex<HashMap<u32, std_mpsc::Sender<Transfer>>>,
    events: EventSink,
    /// Transfers someone awaits: the local path of a download, or an error.
    waiters: Mutex<HashMap<u32, oneshot::Sender<Result<Option<String>, String>>>>,
    /// The host continues interrupted transfers (`Features::RESUME`).
    resume: AtomicBool,
    /// The session ended; downloads keep their partial copies.
    ended: AtomicBool,
}

impl FileClient {
    pub fn new(outbox: mpsc::UnboundedSender<ViewerMsg>, events: EventSink) -> Arc<Self> {
        Arc::new(Self {
            outbox,
            next: AtomicU32::new(1),
            pending: Mutex::default(),
            uploads: Mutex::default(),
            downloads: Mutex::default(),
            events,
            waiters: Mutex::default(),
            resume: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        })
    }

    /// Whether the host continues interrupted transfers.
    pub fn set_resume(&self, on: bool) {
        self.resume.store(on, Ordering::Relaxed);
    }

    fn number(&self) -> u32 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    async fn request(&self, op: FileOp) -> Result<FileReply> {
        let req = self.number();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(req, tx);
        if self.outbox.send(ViewerMsg::File { req, op }).is_err() {
            self.pending.lock().unwrap().remove(&req);
            bail!("Die Sitzung ist beendet");
        }
        let reply = tokio::time::timeout(REQUEST_TIMEOUT, rx).await;
        self.pending.lock().unwrap().remove(&req);
        match reply {
            Ok(Ok(reply)) => reply.map_err(|e| anyhow!(e)),
            Ok(Err(_)) => bail!("Die Sitzung ist beendet"),
            Err(_) => bail!("Die Gegenstelle antwortet nicht"),
        }
    }

    async fn done(&self, op: FileOp) -> Result<()> {
        match self.request(op).await? {
            FileReply::Done => Ok(()),
            _ => bail!("Unerwartete Antwort"),
        }
    }

    pub async fn list(&self, path: String) -> Result<Listing> {
        match self.request(FileOp::List { path }).await? {
            FileReply::Listing(listing) => Ok(listing),
            _ => bail!("Unerwartete Antwort"),
        }
    }

    /// Copies local files and folders to the host and puts them on its
    /// clipboard, so Ctrl+V there pastes them. Needs [`Features::FILE_PASTE`].
    ///
    /// [`Features::FILE_PASTE`]: ctxremote_proto::session::Features::FILE_PASTE
    pub async fn paste_to_host(self: &Arc<Self>, local: Vec<PathBuf>) -> Result<()> {
        let dir = match self.request(FileOp::PasteDir).await? {
            FileReply::Path(dir) => dir,
            _ => bail!("Unerwartete Antwort"),
        };
        for path in local {
            let id = self.number();
            let wait = self.waiter(id);
            self.start_upload(id, path, dir.clone()).await?;
            Self::finished(wait).await?;
        }
        self.done(FileOp::ClipboardFromDir { dir }).await
    }

    /// Fetches files copied at the host into a local folder for pasting;
    /// returns where they landed, for this computer's clipboard.
    pub async fn fetch_to_clipboard(self: &Arc<Self>, remote: Vec<String>) -> Result<Vec<PathBuf>> {
        let root = super::UserContext::current().paste_root();
        let dir = tokio::task::spawn_blocking(move || super::paste_dir(&root)).await??;
        let mut landed = Vec::new();
        for path in remote {
            let id = self.number();
            let wait = self.waiter(id);
            self.start_download(id, path, dir.clone()).await?;
            if let Some(local) = Self::finished(wait).await? {
                landed.push(PathBuf::from(local));
            }
        }
        Ok(landed)
    }

    fn waiter(&self, id: u32) -> oneshot::Receiver<Result<Option<String>, String>> {
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(id, tx);
        rx
    }

    async fn finished(wait: oneshot::Receiver<Result<Option<String>, String>>) -> Result<Option<String>> {
        match wait.await {
            Ok(result) => result.map_err(|e| anyhow!(e)),
            Err(_) => bail!("Die Sitzung ist beendet"),
        }
    }

    /// Reports a transfer's end to listeners and to whoever awaits it.
    fn finish(&self, event: TransferEvent) {
        let outcome = match &event {
            TransferEvent::Finished { id, path } => Some((*id, Ok(path.clone()))),
            TransferEvent::Failed { id, message } => Some((*id, Err(message.clone()))),
            TransferEvent::Progress { .. } => None,
        };
        (self.events)(event);
        if let Some((id, result)) = outcome {
            if let Some(tx) = self.waiters.lock().unwrap().remove(&id) {
                let _ = tx.send(result);
            }
        }
    }

    pub async fn create_dir(&self, path: String) -> Result<()> {
        self.done(FileOp::CreateDir { path }).await
    }

    pub async fn rename(&self, path: String, name: String) -> Result<()> {
        self.done(FileOp::Rename { path, name }).await
    }

    pub async fn delete(&self, paths: Vec<String>) -> Result<()> {
        self.done(FileOp::Delete { paths }).await
    }

    /// Sends the local file or folder `local` into the host's folder `remote_dir`.
    /// Progress and the outcome arrive as [`TransferEvent`]s under the returned id.
    pub async fn upload(self: &Arc<Self>, local: PathBuf, remote_dir: String) -> Result<u32> {
        let id = self.number();
        self.start_upload(id, local, remote_dir).await?;
        Ok(id)
    }

    async fn start_upload(self: &Arc<Self>, id: u32, local: PathBuf, remote_dir: String) -> Result<()> {
        // A single file asks the host first how much of it is already there.
        let single = std::fs::metadata(&local).ok().filter(|m| m.is_file()).map(|m| m.len());
        let name = local.file_name().map(|n| n.to_string_lossy().into_owned());
        if let (true, Some(size), Some(name)) = (self.resume.load(Ordering::Relaxed), single, name) {
            let op = FileOp::UploadFrom { id, dir: remote_dir.clone(), name, size };
            let (have, check) = match self.request(op).await {
                Ok(FileReply::Offset { at, check }) => (at, check),
                Ok(_) => (0, 0),
                Err(e) => {
                    self.waiters.lock().unwrap().remove(&id);
                    return Err(e);
                }
            };
            let source = local.clone();
            let opened = tokio::task::spawn_blocking(move || Outgoing::open_from(&source, have, size, check)).await?;
            match opened {
                // The host expects exactly the rest.
                Ok((outgoing, from)) if from == have => return self.run_upload(id, outgoing),
                // The host's piece is of another file (or this one changed):
                // that attempt ends, and the file goes again from the start.
                Ok(_) => {
                    let _ = self.outbox.send(ViewerMsg::Transfer { id, msg: Transfer::Failed("Datei geändert".into()) });
                }
                Err(e) => {
                    let _ = self.outbox.send(ViewerMsg::Transfer { id, msg: Transfer::Failed(format!("{e:#}")) });
                    self.waiters.lock().unwrap().remove(&id);
                    return Err(e);
                }
            }
        }
        let opened = tokio::task::spawn_blocking(move || Outgoing::open(&local)).await?;
        let outgoing = match opened {
            Ok(outgoing) => outgoing,
            Err(e) => {
                self.waiters.lock().unwrap().remove(&id);
                return Err(e);
            }
        };
        if let Err(e) = self.done(FileOp::Upload { id, dir: remote_dir }).await {
            self.waiters.lock().unwrap().remove(&id);
            return Err(e);
        }
        self.run_upload(id, outgoing)
    }

    /// Sends an announced upload on a thread of its own.
    fn run_upload(self: &Arc<Self>, id: u32, mut outgoing: Outgoing) -> Result<()> {
        let upload = Arc::new(Upload {
            acked: Mutex::new(0),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
            total: outgoing.total(),
            reported: Mutex::new(Instant::now()),
        });
        self.uploads.lock().unwrap().insert(id, upload.clone());
        let this = self.clone();
        std::thread::Builder::new().name("ctxremote-upload".into()).spawn(move || {
            if let Err(e) = this.send_upload(id, &upload, &mut outgoing) {
                if !upload.stopped.load(Ordering::SeqCst) {
                    let _ = this.outbox.send(ViewerMsg::Transfer { id, msg: Transfer::Failed(format!("{e:#}")) });
                    this.fail(id, format!("{e:#}"));
                }
            }
        })?;
        Ok(())
    }

    fn send_upload(&self, id: u32, upload: &Upload, outgoing: &mut Outgoing) -> Result<()> {
        let mut sent = 0u64;
        while let Some(msg) = outgoing.next_msg()? {
            if let Transfer::Data(data) = &msg {
                sent += data.len() as u64;
                // Keep at most a window unacknowledged, so memory and the
                // connection's queue stay small however large the upload.
                let mut acked = upload.acked.lock().unwrap();
                let mut waited = Instant::now();
                let mut last = *acked;
                while sent.saturating_sub(*acked) > WINDOW && !upload.stopped.load(Ordering::SeqCst) {
                    acked = upload.wake.wait_timeout(acked, Duration::from_millis(500)).unwrap().0;
                    if *acked != last {
                        last = *acked;
                        waited = Instant::now();
                    } else if waited.elapsed() > ACK_TIMEOUT {
                        bail!("Die Gegenstelle bestätigt keine Daten mehr");
                    }
                }
            }
            if upload.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }
            self.outbox
                .send(ViewerMsg::Transfer { id, msg })
                .map_err(|_| anyhow!("Die Sitzung ist beendet"))?;
        }
        Ok(())
    }

    /// Fetches the host's file or folder `remote` into the local folder `local_dir`.
    pub async fn download(self: &Arc<Self>, remote: String, local_dir: PathBuf) -> Result<u32> {
        let id = self.number();
        self.start_download(id, remote, local_dir).await?;
        Ok(id)
    }

    async fn start_download(self: &Arc<Self>, id: u32, remote: String, local_dir: PathBuf) -> Result<()> {
        // An interrupted copy of the same name here: ask the host to continue it.
        let name = remote.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
        let part = if self.resume.load(Ordering::Relaxed) && !name.is_empty() { super::find_part(&local_dir, &name) } else { None };
        if let Some((size, have)) = part {
            return self.continue_download(id, remote, local_dir, name, size, have).await;
        }
        let incoming = match Incoming::new(&local_dir) {
            Ok(incoming) => incoming,
            Err(e) => {
                self.waiters.lock().unwrap().remove(&id);
                return Err(e);
            }
        };
        let (tx, rx) = std_mpsc::channel();
        self.downloads.lock().unwrap().insert(id, tx);
        let this = self.clone();
        std::thread::Builder::new()
            .name("ctxremote-download".into())
            .spawn(move || this.receive_download(id, incoming, rx))?;
        if let Err(e) = self.done(FileOp::Download { id, path: remote }).await {
            // Dropping the sender ends the thread without an event.
            self.downloads.lock().unwrap().remove(&id);
            self.waiters.lock().unwrap().remove(&id);
            return Err(e);
        }
        Ok(())
    }

    async fn continue_download(
        self: &Arc<Self>,
        id: u32,
        remote: String,
        local_dir: PathBuf,
        name: String,
        size: u64,
        have: u64,
    ) -> Result<()> {
        let mut incoming = match Incoming::new(&local_dir) {
            Ok(incoming) => incoming,
            Err(e) => {
                self.waiters.lock().unwrap().remove(&id);
                return Err(e);
            }
        };
        // Registered first: content arriving before the answer waits in the channel.
        let (tx, rx) = std_mpsc::channel();
        self.downloads.lock().unwrap().insert(id, tx);
        let part = local_dir.join(super::part_name(&name, size));
        let check = super::tail_check(&part, have).unwrap_or(0);
        let from = match self.request(FileOp::DownloadFrom { id, path: remote, offset: have, size, check }).await {
            Ok(FileReply::Offset { at, .. }) => at,
            Ok(_) => 0,
            Err(e) => {
                self.downloads.lock().unwrap().remove(&id);
                self.waiters.lock().unwrap().remove(&id);
                return Err(e);
            }
        };
        if from > 0 {
            incoming.resume_at(from);
        } else {
            // The file there changed: the old piece is of no use.
            let _ = std::fs::remove_file(local_dir.join(super::part_name(&name, size)));
        }
        let this = self.clone();
        std::thread::Builder::new()
            .name("ctxremote-download".into())
            .spawn(move || this.receive_download(id, incoming, rx))?;
        Ok(())
    }

    fn receive_download(&self, id: u32, mut incoming: Incoming, rx: std_mpsc::Receiver<Transfer>) {
        let mut reported = Instant::now();
        loop {
            let Ok(msg) = rx.recv() else {
                // Cancelled or the session ended. A cancel removes the partial
                // copy; a lost session keeps it to continue later.
                if self.ended.load(Ordering::SeqCst) && self.resume.load(Ordering::Relaxed) {
                    incoming.keep_partial();
                }
                return;
            };
            // Messages still queued after a cancel are not applied.
            if !self.downloads.lock().unwrap().contains_key(&id) {
                if self.ended.load(Ordering::SeqCst) && self.resume.load(Ordering::Relaxed) {
                    incoming.keep_partial();
                }
                return;
            }
            match incoming.apply(msg) {
                Ok(true) => {
                    if self.downloads.lock().unwrap().remove(&id).is_none() {
                        // Cancelled while the last chunk was written: honour the cancel.
                        if let Some(path) = incoming.finished() {
                            let _ = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
                        }
                        return;
                    }
                    self.progress(id, incoming.written(), incoming.total());
                    let path = incoming.finished().map(|p| p.to_string_lossy().into_owned());
                    self.finish(TransferEvent::Finished { id, path });
                    // Keeps the result: dropping a finished transfer removes nothing.
                    return;
                }
                Ok(false) => {
                    if reported.elapsed() >= PROGRESS_INTERVAL {
                        reported = Instant::now();
                        self.progress(id, incoming.written(), incoming.total());
                    }
                }
                Err(e) => {
                    if self.downloads.lock().unwrap().remove(&id).is_some() {
                        let _ = self.outbox.send(ViewerMsg::Transfer { id, msg: Transfer::Cancel });
                    }
                    self.fail(id, format!("{e:#}"));
                    return;
                }
            }
        }
    }

    pub fn cancel(&self, id: u32) {
        let upload = self.uploads.lock().unwrap().remove(&id);
        let download = self.downloads.lock().unwrap().remove(&id);
        if let Some(upload) = &upload {
            upload.stop();
        }
        if upload.is_some() || download.is_some() {
            let _ = self.outbox.send(ViewerMsg::Transfer { id, msg: Transfer::Cancel });
            self.fail(id, "Abgebrochen".into());
        }
    }

    /// Routes a reply from the host.
    pub fn reply(&self, req: u32, result: Result<FileReply, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&req) {
            let _ = tx.send(result);
        }
    }

    /// Routes a transfer message from the host: download content, or the
    /// outcome of an upload.
    pub fn transfer(&self, id: u32, msg: Transfer) {
        if let Some(tx) = self.downloads.lock().unwrap().get(&id) {
            let _ = tx.send(msg);
            return;
        }
        let Some(upload) = self.uploads.lock().unwrap().remove(&id) else { return };
        upload.stop();
        match msg {
            Transfer::End => {
                self.progress(id, upload.total, upload.total);
                self.finish(TransferEvent::Finished { id, path: None });
            }
            Transfer::Failed(message) => self.fail(id, message),
            _ => self.fail(id, "Unerwartete Antwort".into()),
        }
    }

    /// Routes an upload acknowledgement from the host.
    pub fn ack(&self, id: u32, bytes: u64) {
        let Some(upload) = self.uploads.lock().unwrap().get(&id).cloned() else { return };
        *upload.acked.lock().unwrap() = bytes;
        upload.wake.notify_all();
        let mut reported = upload.reported.lock().unwrap();
        if reported.elapsed() >= PROGRESS_INTERVAL {
            *reported = Instant::now();
            self.progress(id, bytes, upload.total);
        }
    }

    /// The session ended: every running transfer fails.
    pub fn closed(&self) {
        self.ended.store(true, Ordering::SeqCst);
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err("Die Sitzung ist beendet".into()));
        }
        let uploads: Vec<_> = self.uploads.lock().unwrap().drain().collect();
        let downloads: Vec<_> = self.downloads.lock().unwrap().drain().map(|(id, _)| id).collect();
        let message = if self.resume.load(Ordering::Relaxed) {
            "Verbindung getrennt. Eine einzelne Datei wird beim nächsten Übertragen an dieser Stelle fortgesetzt."
        } else {
            "Verbindung getrennt"
        };
        for (id, upload) in uploads {
            upload.stop();
            self.fail(id, message.into());
        }
        for id in downloads {
            self.fail(id, message.into());
        }
    }

    fn progress(&self, id: u32, done: u64, total: u64) {
        (self.events)(TransferEvent::Progress { id, done, total });
    }

    fn fail(&self, id: u32, message: String) {
        self.finish(TransferEvent::Failed { id, message });
    }
}

/// The local default target for downloads.
pub fn default_download_dir() -> Option<PathBuf> {
    super::UserContext::current().downloads()
}

#[cfg(test)]
mod tests {
    //! Host and viewer wired together through channels, no network.

    use std::path::Path;

    use ctxremote_proto::session::{EntryKind, HostMsg};

    use super::super::find_part;
    use super::super::service::FileService;
    use super::*;

    const WAIT: Duration = Duration::from_secs(30);

    struct Rig {
        client: Arc<FileClient>,
        events: mpsc::UnboundedReceiver<TransferEvent>,
        /// While set, upload acknowledgements from the host are dropped.
        hold_acks: Arc<AtomicBool>,
    }

    fn rig() -> Rig {
        let (viewer_tx, mut viewer_rx) = mpsc::unbounded_channel::<ViewerMsg>();
        let (host_tx, mut host_rx) = mpsc::channel::<HostMsg>(2);
        let (event_tx, events) = mpsc::unbounded_channel();
        let client = FileClient::new(viewer_tx, Arc::new(move |e| drop(event_tx.send(e))));
        let service = FileService::start(host_tx).unwrap();
        tokio::spawn(async move {
            while let Some(msg) = viewer_rx.recv().await {
                service.handle(msg);
            }
        });
        let hold_acks = Arc::new(AtomicBool::new(false));
        let (route, hold) = (client.clone(), hold_acks.clone());
        tokio::spawn(async move {
            while let Some(msg) = host_rx.recv().await {
                match msg {
                    HostMsg::FileReply { req, result } => route.reply(req, result),
                    HostMsg::Transfer { id, msg } => route.transfer(id, msg),
                    HostMsg::TransferAck { id, bytes } => {
                        if !hold.load(Ordering::SeqCst) {
                            route.ack(id, bytes);
                        }
                    }
                    _ => {}
                }
            }
        });
        Rig { client, events, hold_acks }
    }

    impl Rig {
        /// The final event of transfer `id` (`Finished` or `Failed`).
        async fn outcome(&mut self, id: u32) -> TransferEvent {
            tokio::time::timeout(WAIT, async {
                loop {
                    match self.events.recv().await.expect("event channel closed") {
                        TransferEvent::Progress { .. } => {}
                        e @ (TransferEvent::Finished { id: i, .. } | TransferEvent::Failed { id: i, .. }) if i == id => {
                            return e
                        }
                        _ => {}
                    }
                }
            })
            .await
            .expect("timeout waiting for the end of the transfer")
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctxremote-e2e-{label}-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn s(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    fn pattern(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + seed) % 251) as u8).collect()
    }

    /// A tree with an empty folder, a nested file, a small and an empty file, and a file > 3 MiB.
    fn make_tree(root: &Path) -> (PathBuf, Vec<u8>) {
        let tree = root.join("Projekt");
        std::fs::create_dir_all(tree.join("leer")).unwrap();
        std::fs::create_dir_all(tree.join("sub/tief")).unwrap();
        let big = pattern(3 * 1024 * 1024 + 12345, 7);
        std::fs::write(tree.join("sub/big.bin"), &big).unwrap();
        std::fs::write(tree.join("sub/tief/x.txt"), "tief").unwrap();
        std::fs::write(tree.join("a.txt"), "hallo").unwrap();
        std::fs::write(tree.join("null.txt"), "").unwrap();
        (tree, big)
    }

    fn assert_tree(copy: &Path, big: &[u8]) {
        assert_eq!(std::fs::read(copy.join("sub/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read_to_string(copy.join("sub/tief/x.txt")).unwrap(), "tief");
        assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "hallo");
        assert_eq!(std::fs::read(copy.join("null.txt")).unwrap(), b"");
        assert!(copy.join("leer").is_dir());
        assert_eq!(std::fs::read_dir(copy.join("leer")).unwrap().count(), 0);
    }

    fn is_empty(dir: &Path) -> bool {
        std::fs::read_dir(dir).unwrap().count() == 0
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lists_top_level_and_folders() {
        let rig = rig();
        let top = rig.client.list(String::new()).await.unwrap();
        assert!(top.parent.is_none());
        assert!(top.entries.iter().any(|e| e.kind == EntryKind::Drive));

        let dir = temp_dir("list");
        std::fs::create_dir(dir.join("Zeta")).unwrap();
        std::fs::write(dir.join("alpha.txt"), "hello").unwrap();
        let listing = rig.client.list(s(&dir)).await.unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| (e.name.as_str(), e.kind, e.size)).collect();
        assert_eq!(names, [("Zeta", EntryKind::Dir, 0), ("alpha.txt", EntryKind::File, 5)]);
        assert_eq!(listing.parent.as_deref(), dir.parent().map(|p| p.to_str().unwrap()));

        assert!(rig.client.list(s(&dir.join("gibt-es-nicht"))).await.is_err());
        assert!(rig.client.list("relativ".into()).await.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_rename_delete() {
        let rig = rig();
        let dir = temp_dir("ops");

        rig.client.create_dir(s(&dir.join("a"))).await.unwrap();
        assert!(dir.join("a").is_dir());
        assert!(rig.client.create_dir(s(&dir.join("a"))).await.is_err(), "already exists");

        rig.client.create_dir(s(&dir.join("b"))).await.unwrap();
        assert!(rig.client.rename(s(&dir.join("a")), "b".into()).await.is_err());
        assert!(dir.join("a").is_dir() && dir.join("b").is_dir());
        assert!(rig.client.rename(s(&dir.join("a")), "../x".into()).await.is_err());

        rig.client.rename(s(&dir.join("a")), "c".into()).await.unwrap();
        assert!(!dir.join("a").exists() && dir.join("c").is_dir());

        std::fs::write(dir.join("c/file.txt"), "x").unwrap();
        std::fs::write(dir.join("single.txt"), "y").unwrap();
        rig.client.delete(vec![s(&dir.join("c")), s(&dir.join("single.txt"))]).await.unwrap();
        assert!(!dir.join("c").exists() && !dir.join("single.txt").exists());
        assert!(dir.join("b").is_dir());
        assert!(rig.client.delete(vec![s(&dir.join("weg"))]).await.is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn uploads_a_tree_twice() {
        let mut rig = rig();
        let src = temp_dir("up-src");
        let dst = temp_dir("up-dst");
        let (tree, big) = make_tree(&src);

        let id = rig.client.upload(tree.clone(), s(&dst)).await.unwrap();
        match rig.outcome(id).await {
            TransferEvent::Finished { path, .. } => assert_eq!(path, None),
            e => panic!("unexpected {e:?}"),
        }
        assert_tree(&dst.join("Projekt"), &big);

        let id = rig.client.upload(tree.clone(), s(&dst)).await.unwrap();
        assert!(matches!(rig.outcome(id).await, TransferEvent::Finished { .. }));
        assert_tree(&dst.join("Projekt (2)"), &big);

        // A single file works, too.
        let id = rig.client.upload(tree.join("a.txt"), s(&dst)).await.unwrap();
        assert!(matches!(rig.outcome(id).await, TransferEvent::Finished { .. }));
        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "hallo");

        // No leftovers of temporary names.
        let names: Vec<_> = std::fs::read_dir(&dst).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".ctxpart")), "{names:?}");

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_a_file_and_a_folder() {
        let mut rig = rig();
        let src = temp_dir("down-src");
        let dst = temp_dir("down-dst");
        let (tree, big) = make_tree(&src);

        let id = rig.client.download(s(&tree.join("sub/big.bin")), dst.clone()).await.unwrap();
        match rig.outcome(id).await {
            TransferEvent::Finished { path, .. } => assert_eq!(path, Some(s(&dst.join("big.bin")))),
            e => panic!("unexpected {e:?}"),
        }
        assert_eq!(std::fs::read(dst.join("big.bin")).unwrap(), big);

        let id = rig.client.download(s(&tree), dst.clone()).await.unwrap();
        match rig.outcome(id).await {
            TransferEvent::Finished { path, .. } => assert_eq!(path, Some(s(&dst.join("Projekt")))),
            e => panic!("unexpected {e:?}"),
        }
        assert_tree(&dst.join("Projekt"), &big);

        // Again: a free name, the first copy stays.
        let id = rig.client.download(s(&tree), dst.clone()).await.unwrap();
        match rig.outcome(id).await {
            TransferEvent::Finished { path, .. } => assert_eq!(path, Some(s(&dst.join("Projekt (2)")))),
            e => panic!("unexpected {e:?}"),
        }
        assert_tree(&dst.join("Projekt (2)"), &big);

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failing_requests_report_errors() {
        let rig = rig();
        let src = temp_dir("err-src");
        let dst = temp_dir("err-dst");
        std::fs::write(src.join("f.txt"), "x").unwrap();

        assert!(rig.client.download(s(&src.join("gibt-es-nicht")), dst.clone()).await.is_err());
        assert!(rig.client.download(s(&src.join("f.txt")), dst.join("kein-ordner")).await.is_err());
        assert!(rig.client.upload(src.join("f.txt"), s(&dst.join("kein-ordner"))).await.is_err());
        assert!(rig.client.upload(src.join("gibt-es-nicht"), s(&dst)).await.is_err());

        // Nothing stays registered or on disk.
        assert!(rig.client.uploads.lock().unwrap().is_empty());
        assert!(rig.client.downloads.lock().unwrap().is_empty());
        assert!(is_empty(&dst));

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_download_leaves_nothing() {
        let mut rig = rig();
        let src = temp_dir("cancel-src");
        let dst = temp_dir("cancel-dst");
        std::fs::write(src.join("huge.bin"), pattern(20 * 1024 * 1024, 3)).unwrap();

        let id = rig.client.download(s(&src.join("huge.bin")), dst.clone()).await.unwrap();
        rig.client.cancel(id);
        match rig.outcome(id).await {
            TransferEvent::Failed { message, .. } => assert_eq!(message, "Abgebrochen"),
            e => panic!("unexpected {e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(is_empty(&dst), "partial download remains: {:?}", std::fs::read_dir(&dst).unwrap().collect::<Vec<_>>());
        assert!(rig.client.downloads.lock().unwrap().is_empty());

        // The host stopped sending: the session is still usable and no stray event arrives.
        rig.client.list(s(&src)).await.unwrap();
        while let Ok(e) = rig.events.try_recv() {
            assert!(!matches!(e, TransferEvent::Finished { .. }), "{e:?}");
        }

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_upload_leaves_nothing() {
        let mut rig = rig();
        let src = temp_dir("cancelup-src");
        let dst = temp_dir("cancelup-dst");
        std::fs::write(src.join("huge.bin"), pattern(20 * 1024 * 1024, 5)).unwrap();

        rig.hold_acks.store(true, Ordering::SeqCst);
        let id = rig.client.upload(src.join("huge.bin"), s(&dst)).await.unwrap();
        // Without acknowledgements the sender stalls after one window.
        tokio::time::sleep(Duration::from_millis(300)).await;
        rig.client.cancel(id);
        match rig.outcome(id).await {
            TransferEvent::Failed { message, .. } => assert_eq!(message, "Abgebrochen"),
            e => panic!("unexpected {e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(is_empty(&dst), "partial upload remains: {:?}", std::fs::read_dir(&dst).unwrap().collect::<Vec<_>>());

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    /// A download cut off by an earlier session continues: only the rest travels.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_interrupted_download_continues() {
        let src = temp_dir("resume-src");
        let dst = temp_dir("resume-dst");
        let data = pattern(5 * 1024 * 1024 + 77, 3);
        std::fs::write(src.join("film.bin"), &data).unwrap();
        let size = data.len() as u64;
        let have = 2 * 1024 * 1024 + 5;
        // What the earlier session left behind.
        std::fs::write(dst.join(super::super::part_name("film.bin", size)), &data[..have as usize]).unwrap();

        let mut rig = rig();
        rig.client.set_resume(true);
        let id = rig.client.download(s(&src.join("film.bin")), dst.clone()).await.unwrap();
        let mut total = None;
        let outcome = tokio::time::timeout(WAIT, async {
            loop {
                match rig.events.recv().await.unwrap() {
                    TransferEvent::Progress { id: i, total: t, .. } if i == id => total = Some(t),
                    e @ (TransferEvent::Finished { id: i, .. } | TransferEvent::Failed { id: i, .. }) if i == id => return e,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(outcome, TransferEvent::Finished { .. }), "{outcome:?}");
        assert_eq!(total, Some(size - have));
        assert_eq!(std::fs::read(dst.join("film.bin")).unwrap(), data);
        assert!(find_part(&dst, "film.bin").is_none());

        // The host's file changed meanwhile: the old piece goes, it starts over.
        let changed = pattern(3 * 1024 * 1024, 4);
        std::fs::write(src.join("neu.bin"), &changed).unwrap();
        std::fs::write(dst.join(super::super::part_name("neu.bin", 999_999_999)), b"alt").unwrap();
        let id = rig.client.download(s(&src.join("neu.bin")), dst.clone()).await.unwrap();
        assert!(matches!(rig.outcome(id).await, TransferEvent::Finished { .. }));
        assert_eq!(std::fs::read(dst.join("neu.bin")).unwrap(), changed);
        assert!(find_part(&dst, "neu.bin").is_none());

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    /// An upload continues the host's interrupted copy.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_interrupted_upload_continues() {
        let src = temp_dir("resume-up-src");
        let dst = temp_dir("resume-up-dst");
        let data = pattern(4 * 1024 * 1024 + 3, 5);
        std::fs::write(src.join("backup.zip"), &data).unwrap();
        let size = data.len() as u64;
        std::fs::write(dst.join(super::super::part_name("backup.zip", size)), &data[..1_000_000]).unwrap();

        let mut rig = rig();
        rig.client.set_resume(true);
        let id = rig.client.upload(src.join("backup.zip"), s(&dst)).await.unwrap();
        assert!(matches!(rig.outcome(id).await, TransferEvent::Finished { .. }));
        assert_eq!(std::fs::read(dst.join("backup.zip")).unwrap(), data);
        assert!(find_part(&dst, "backup.zip").is_none());
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closing_fails_running_transfers() {
        let mut rig = rig();
        let src = temp_dir("close-src");
        let dst = temp_dir("close-dst");
        std::fs::write(src.join("huge.bin"), pattern(20 * 1024 * 1024, 9)).unwrap();

        // An upload that cannot progress (no acknowledgements) ...
        rig.hold_acks.store(true, Ordering::SeqCst);
        let up = rig.client.upload(src.join("huge.bin"), s(&dst)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(rig.client.uploads.lock().unwrap().contains_key(&up));
        rig.client.closed();
        match rig.outcome(up).await {
            TransferEvent::Failed { message, .. } => assert_eq!(message, "Verbindung getrennt"),
            e => panic!("unexpected {e:?}"),
        }
        assert!(rig.client.uploads.lock().unwrap().is_empty());

        // ... and a download that is just starting.
        let down_dir = temp_dir("close-down");
        let down = rig.client.download(s(&src.join("huge.bin")), down_dir.clone()).await.unwrap();
        rig.client.closed();
        match rig.outcome(down).await {
            TransferEvent::Failed { message, .. } => assert_eq!(message, "Verbindung getrennt"),
            e => panic!("unexpected {e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(is_empty(&down_dir), "partial download remains");
        assert!(rig.client.downloads.lock().unwrap().is_empty());

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
        std::fs::remove_dir_all(&down_dir).unwrap();
    }

    /// Pasting to the host: the copies land in a fresh folder there; putting
    /// them on the clipboard is the agent's part, which this rig lacks.
    #[tokio::test]
    async fn paste_uploads_into_a_fresh_folder() {
        let rig = rig();
        let src = temp_dir("paste-src");
        let (tree, big) = make_tree(&src);
        let single = src.join("notiz.txt");
        std::fs::write(&single, "hallo").unwrap();
        let result = rig.client.paste_to_host(vec![tree, single]).await;
        let error = result.expect_err("ohne Agent keine Zwischenablage");
        assert!(format!("{error:#}").contains("Zwischenablage"), "{error:#}");
        let root = super::super::UserContext::current().paste_root();
        let newest = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .max_by_key(|e| e.metadata().unwrap().modified().unwrap())
            .unwrap()
            .path();
        assert_eq!(std::fs::read(newest.join("Projekt/sub/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read_to_string(newest.join("notiz.txt")).unwrap(), "hallo");
        assert_eq!(super::super::entries_of(&newest).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&newest);
    }

    /// Fetching files copied at the host: each lands locally, paths come back.
    #[tokio::test]
    async fn fetch_lands_files_for_the_clipboard() {
        let rig = rig();
        let src = temp_dir("fetch-src");
        let (tree, big) = make_tree(&src);
        let landed = rig.client.fetch_to_clipboard(vec![s(&tree)]).await.unwrap();
        assert_eq!(landed.len(), 1);
        assert_eq!(std::fs::read(landed[0].join("sub/big.bin")).unwrap(), big);
        let _ = std::fs::remove_dir_all(landed[0].parent().unwrap());
    }
}