//! Command lines at the host (see [`TerminalMsg`]): one shell per terminal,
//! in a pseudo terminal, as the user signed in at the computer.
//!
//! Runs in the agent like file access. Output is read on a thread of its own
//! and goes out through the agent's small outbox, so a shell printing fast
//! is slowed down by the connection instead of filling memory. Keys are
//! written on another thread: a shell that does not read cannot stall the
//! session.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc as std_mpsc;

use anyhow::Result;
use ctxremote_proto::session::{HostMsg, TerminalMsg};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(windows, all(unix, not(any(target_os = "android", target_os = "ios"))))))]
mod platform {
    pub fn start(_cols: u16, _rows: u16) -> anyhow::Result<super::Started> {
        anyhow::bail!("Auf diesem Gerät gibt es kein Terminal")
    }
}

/// Shells per session; more are refused.
const MAX_OPEN: usize = 8;

/// A shell as the platform starts it.
pub(crate) struct Started {
    pub input: Box<dyn Write + Send>,
    pub output: Box<dyn Read + Send>,
    /// Resizes the window; dropping it ends the shell.
    pub control: Box<dyn Control>,
    /// Runs once the output ended, on the reading thread: collects the process.
    pub reap: Box<dyn FnOnce() + Send>,
    /// What runs, for the viewer: e.g. "bash als max auf werkstatt".
    pub what: String,
}

pub(crate) trait Control: Send {
    fn resize(&self, cols: u16, rows: u16) -> Result<()>;
}

struct Open {
    keys: std_mpsc::Sender<Vec<u8>>,
    control: Box<dyn Control>,
}

/// The shells of one session; dropping it ends them all.
pub struct Terminals {
    outbox: mpsc::Sender<HostMsg>,
    open: HashMap<u32, Open>,
}

impl Terminals {
    pub fn new(outbox: mpsc::Sender<HostMsg>) -> Self {
        Self { outbox, open: HashMap::new() }
    }

    /// Fails only when the outbox is gone.
    pub async fn handle(&mut self, msg: TerminalMsg) -> Result<()> {
        match msg {
            TerminalMsg::Open { id, cols, rows } => {
                let started = if self.open.contains_key(&id) {
                    Err("Dieses Terminal läuft schon".to_string())
                } else if self.open.len() >= MAX_OPEN {
                    Err(format!("Höchstens {MAX_OPEN} Terminals je Sitzung"))
                } else {
                    platform::start(size(cols), size(rows)).map_err(|e| format!("{e:#}"))
                };
                match started {
                    Ok(started) => {
                        info!(id, what = %started.what, "Terminal geöffnet");
                        // First, so no output can overtake it.
                        self.outbox.send(HostMsg::Terminal(TerminalMsg::Opened { id, result: Ok(started.what) })).await?;
                        let keys = spawn_writer(started.input);
                        spawn_reader(id, started.output, started.reap, self.outbox.clone());
                        self.open.insert(id, Open { keys, control: started.control });
                    }
                    Err(e) => {
                        warn!("Terminal nicht gestartet: {e}");
                        self.outbox.send(HostMsg::Terminal(TerminalMsg::Opened { id, result: Err(e) })).await?;
                    }
                }
            }
            TerminalMsg::Data { id, data } => {
                if let Some(open) = self.open.get(&id) {
                    if open.keys.send(data).is_err() {
                        self.open.remove(&id);
                    }
                }
            }
            TerminalMsg::Resize { id, cols, rows } => {
                if let Some(open) = self.open.get(&id) {
                    if let Err(e) = open.control.resize(size(cols), size(rows)) {
                        debug!("Terminalgröße nicht geändert: {e:#}");
                    }
                }
            }
            // Ends the shell; its reader then reports `Close`.
            TerminalMsg::Close { id } => {
                if self.open.remove(&id).is_some() {
                    info!(id, "Terminal geschlossen");
                }
            }
            TerminalMsg::Opened { .. } => {}
        }
        Ok(())
    }
}

/// Columns and rows within what a pseudo terminal takes.
fn size(n: u16) -> u16 {
    n.clamp(2, 1000)
}

fn spawn_writer(mut input: Box<dyn Write + Send>) -> std_mpsc::Sender<Vec<u8>> {
    let (keys, inbox) = std_mpsc::channel::<Vec<u8>>();
    let spawned = std::thread::Builder::new().name("ctxremote-terminal-in".into()).spawn(move || {
        while let Ok(data) = inbox.recv() {
            if input.write_all(&data).and_then(|()| input.flush()).is_err() {
                break;
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Terminal-Eingabe nicht gestartet: {e}");
    }
    keys
}

fn spawn_reader(id: u32, mut output: Box<dyn Read + Send>, reap: Box<dyn FnOnce() + Send>, outbox: mpsc::Sender<HostMsg>) {
    let spawned = std::thread::Builder::new().name("ctxremote-terminal-out".into()).spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match output.read(&mut buf) {
                // Linux reports the end of the shell as an error (EIO).
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    if outbox.blocking_send(HostMsg::Terminal(TerminalMsg::Data { id, data })).is_err() {
                        break;
                    }
                }
            }
        }
        drop(output);
        reap();
        let _ = outbox.blocking_send(HostMsg::Terminal(TerminalMsg::Close { id }));
    });
    if let Err(e) = spawned {
        warn!("Terminal-Ausgabe nicht gestartet: {e}");
    }
}
