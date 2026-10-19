//! The shell in a pseudo terminal on Mac and Linux. The host runs as the
//! signed-in user there, so the shell does too: their login shell, in their
//! home folder.

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

use super::{Control, Started};

struct Pty {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Control for Pty {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

pub fn start(cols: u16, rows: u16) -> Result<Started> {
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .context("Kein Pseudo-Terminal verfügbar")?;
    let mut command = CommandBuilder::new_default_prog();
    command.env("TERM", "xterm-256color");
    let shell = command.get_shell();
    let mut child = pair.slave.spawn_command(command).with_context(|| format!("{shell} nicht startbar"))?;
    // Only the shell may hold the other end, or the output never ends.
    drop(pair.slave);
    let killer = child.clone_killer();
    let output = pair.master.try_clone_reader()?;
    let input = pair.master.take_writer()?;
    let name = std::path::Path::new(&shell).file_name().map_or(shell.clone(), |n| n.to_string_lossy().into_owned());
    Ok(Started {
        input,
        output,
        control: Box::new(Pty { master: pair.master, killer }),
        reap: Box::new(move || {
            let _ = child.wait();
        }),
        what: format!("{name} als {} auf {}", whoami::username(), whoami::devicename()),
    })
}
