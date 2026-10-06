//! The host side: answers the viewer's file requests on a thread of its own.
//!
//! File access blocks and, in service mode, runs with the signed-in user's
//! rights (see [`UserContext`]), which only works per thread. Downloads are
//! sent one chunk per round, interleaved with incoming requests, so a large
//! download neither blocks the browser nor a cancel.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc as std_mpsc;

use anyhow::Result;
use ctxremote_proto::session::{FileOp, FileReply, HostMsg, Transfer, ViewerMsg};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{Incoming, Outgoing, UserContext};

/// Handle to the file thread; it ends when this is dropped.
pub struct FileService {
    commands: std_mpsc::Sender<ViewerMsg>,
}

impl FileService {
    /// Starts the thread; replies go to `outbox`.
    pub fn start(outbox: mpsc::Sender<HostMsg>) -> Result<Self> {
        let (commands, inbox) = std_mpsc::channel();
        std::thread::Builder::new()
            .name("ctxremote-files".into())
            .spawn(move || run(inbox, outbox))?;
        Ok(Self { commands })
    }

    /// Takes `ViewerMsg::File` and `ViewerMsg::Transfer`; other messages are ignored.
    pub fn handle(&self, msg: ViewerMsg) {
        let _ = self.commands.send(msg);
    }
}

struct State {
    outbox: mpsc::Sender<HostMsg>,
    user: Option<UserContext>,
    downloads: Vec<(u32, Outgoing)>,
    uploads: HashMap<u32, Incoming>,
    /// Uploads begun with `UploadFrom`: their viewer can continue them later.
    resumable: std::collections::HashSet<u32>,
}

fn run(inbox: std_mpsc::Receiver<ViewerMsg>, outbox: mpsc::Sender<HostMsg>) {
    let mut state = State { outbox, user: None, downloads: Vec::new(), uploads: HashMap::new(), resumable: Default::default() };
    serve(&mut state, inbox);
    // The session is gone: uploads still running can be continued later.
    for (id, upload) in state.uploads.iter_mut() {
        if state.resumable.contains(id) {
            upload.keep_partial();
        }
    }
}

fn serve(state: &mut State, inbox: std_mpsc::Receiver<ViewerMsg>) {
    loop {
        // Requests first; with nothing to send, wait for the next one.
        let msg = if state.downloads.is_empty() {
            match inbox.recv() {
                Ok(msg) => Some(msg),
                Err(_) => return,
            }
        } else {
            match inbox.try_recv() {
                Ok(msg) => Some(msg),
                Err(std_mpsc::TryRecvError::Empty) => None,
                Err(std_mpsc::TryRecvError::Disconnected) => return,
            }
        };
        let alive = match msg {
            Some(ViewerMsg::File { req, op }) => state.op(req, op),
            Some(ViewerMsg::Transfer { id, msg }) => state.incoming(id, msg),
            Some(_) => true,
            None => state.send_round(),
        };
        if !alive {
            return;
        }
    }
}

impl State {
    /// `false` once the session is gone.
    fn send(&self, msg: HostMsg) -> bool {
        self.outbox.blocking_send(msg).is_ok()
    }

    /// Takes on the signed-in user's rights on first use; retried until a user is there.
    fn user(&mut self) -> Result<&UserContext> {
        if self.user.is_none() {
            self.user = Some(UserContext::for_host()?);
        }
        Ok(self.user.as_ref().expect("set above"))
    }

    fn op(&mut self, req: u32, op: FileOp) -> bool {
        let result = self.run_op(op).map_err(|e| format!("{e:#}"));
        self.send(HostMsg::FileReply { req, result })
    }

    fn run_op(&mut self, op: FileOp) -> Result<FileReply> {
        let user = self.user()?;
        Ok(match op {
            FileOp::List { path } => FileReply::Listing(super::list(&path, user)?),
            FileOp::CreateDir { path } => {
                super::create_dir(&path)?;
                FileReply::Done
            }
            FileOp::Rename { path, name } => {
                super::rename(&path, &name)?;
                FileReply::Done
            }
            FileOp::Delete { paths } => {
                super::delete(&paths)?;
                FileReply::Done
            }
            FileOp::Download { id, path } => {
                let outgoing = Outgoing::open(Path::new(&path))?;
                info!(%path, bytes = outgoing.total(), "Download gestartet");
                self.downloads.push((id, outgoing));
                FileReply::Done
            }
            FileOp::Upload { id, dir } => {
                let incoming = Incoming::new(Path::new(&dir))?;
                self.uploads.insert(id, incoming);
                FileReply::Done
            }
            FileOp::DownloadFrom { id, path, offset, size, check } => {
                let (outgoing, from) = Outgoing::open_from(Path::new(&path), offset, size, check)?;
                info!(%path, from, bytes = outgoing.total(), "Download fortgesetzt");
                self.downloads.push((id, outgoing));
                FileReply::Offset { at: from, check: 0 }
            }
            FileOp::UploadFrom { id, dir, name, size } => {
                super::valid_name(&name)?;
                let mut incoming = Incoming::new(Path::new(&dir))?;
                let part = Path::new(&dir).join(super::part_name(&name, size));
                // A copy another upload writes right now is not continued.
                let have = std::fs::metadata(&part)
                    .ok()
                    .filter(|m| m.is_file() && m.len() < size && !super::part_active(&part))
                    .map_or(0, |m| m.len());
                let check = if have > 0 { super::tail_check(&part, have).unwrap_or(0) } else { 0 };
                incoming.resume_at(have);
                self.uploads.insert(id, incoming);
                self.resumable.insert(id);
                FileReply::Offset { at: have, check }
            }
            // Created with the user's rights, so the pasted copies are the user's.
            FileOp::PasteDir => FileReply::Path(super::paste_dir(&user.paste_root())?.to_string_lossy().into_owned()),
            // The agent handles it, as it owns the clipboard.
            FileOp::ClipboardFromDir { .. } => anyhow::bail!("Zwischenablage hier nicht verfügbar"),
        })
    }

    /// A message of an upload, or a cancel for a download.
    fn incoming(&mut self, id: u32, msg: Transfer) -> bool {
        if matches!(msg, Transfer::Cancel) && self.uploads.get(&id).is_none() {
            self.downloads.retain(|(d, _)| *d != id);
            return true;
        }
        let Some(upload) = self.uploads.get_mut(&id) else {
            // E.g. data still on its way after a failure; already answered.
            return true;
        };
        let quiet = matches!(msg, Transfer::Cancel | Transfer::Failed(_));
        let data = matches!(msg, Transfer::Data(_));
        match upload.apply(msg) {
            Ok(true) => {
                let upload = self.uploads.remove(&id).expect("present");
                info!(path = %upload.finished().map(|p| p.display().to_string()).unwrap_or_default(), "Upload abgeschlossen");
                self.send(HostMsg::TransferAck { id, bytes: upload.written() })
                    && self.send(HostMsg::Transfer { id, msg: Transfer::End })
            }
            Ok(false) if data => {
                let bytes = upload.written();
                self.send(HostMsg::TransferAck { id, bytes })
            }
            Ok(false) => true,
            Err(e) => {
                // Dropping it removes what was written.
                self.uploads.remove(&id);
                if quiet {
                    return true;
                }
                warn!("Upload fehlgeschlagen: {e:#}");
                self.send(HostMsg::Transfer { id, msg: Transfer::Failed(format!("{e:#}")) })
            }
        }
    }

    /// One message from every running download.
    fn send_round(&mut self) -> bool {
        let mut index = 0;
        while index < self.downloads.len() {
            let id = self.downloads[index].0;
            let (msg, done) = match self.downloads[index].1.next_msg() {
                Ok(Some(msg)) => {
                    let end = matches!(msg, Transfer::End);
                    (Some(msg), end)
                }
                Ok(None) => (None, true),
                Err(e) => (Some(Transfer::Failed(format!("{e:#}"))), true),
            };
            if let Some(msg) = msg {
                if !self.send(HostMsg::Transfer { id, msg }) {
                    return false;
                }
            }
            if done {
                self.downloads.remove(index);
            } else {
                index += 1;
            }
        }
        true
    }
}
