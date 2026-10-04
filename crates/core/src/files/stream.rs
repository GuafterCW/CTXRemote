//! Reading a file or folder tree into [`Transfer`] messages and writing them back.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::Transfer;

use super::{free_name, join_relative, valid_name, CHUNK};

enum Item {
    Dir { rel: String },
    File { rel: String, path: PathBuf, size: u64 },
}

/// The sending side of one transfer.
pub struct Outgoing {
    items: std::vec::IntoIter<Item>,
    total: u64,
    started: bool,
    ended: bool,
    current: Option<(File, u64)>,
    buffer: Vec<u8>,
}

impl Outgoing {
    /// Collects `path` (a file, or a folder with everything below it). Links are
    /// skipped so a loop cannot make the transfer endless.
    pub fn open(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path).with_context(|| format!("{} nicht gefunden", path.display()))?;
        let name = path
            .file_name()
            .context("Ein ganzes Laufwerk kann nicht übertragen werden")?
            .to_string_lossy()
            .into_owned();
        let mut items = Vec::new();
        if meta.is_dir() {
            items.push(Item::Dir { rel: name.clone() });
            collect(path, &name, &mut items)?;
        } else {
            items.push(Item::File { rel: name, path: path.to_path_buf(), size: meta.len() });
        }
        let total = items.iter().map(|i| if let Item::File { size, .. } = i { *size } else { 0 }).sum();
        Ok(Self { items: items.into_iter(), total, started: false, ended: false, current: None, buffer: Vec::new() })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// The next message, `None` after `End`. Reads at most one chunk.
    pub fn next_msg(&mut self) -> Result<Option<Transfer>> {
        if !self.started {
            self.started = true;
            return Ok(Some(Transfer::Start { total: self.total }));
        }
        if let Some((file, remaining)) = &mut self.current {
            if *remaining > 0 {
                let want = (*remaining).min(CHUNK as u64) as usize;
                self.buffer.resize(want, 0);
                let read = file.read(&mut self.buffer)?;
                if read == 0 {
                    bail!("Datei wurde während der Übertragung verkürzt");
                }
                *remaining -= read as u64;
                return Ok(Some(Transfer::Data(self.buffer[..read].to_vec())));
            }
            self.current = None;
        }
        match self.items.next() {
            Some(Item::Dir { rel }) => Ok(Some(Transfer::Dir { rel })),
            Some(Item::File { rel, path, size }) => {
                let file = File::open(&path).with_context(|| format!("{} kann nicht gelesen werden", path.display()))?;
                self.current = Some((file, size));
                Ok(Some(Transfer::File { rel, size }))
            }
            None if !self.ended => {
                self.ended = true;
                Ok(Some(Transfer::End))
            }
            None => Ok(None),
        }
    }
}

fn collect(dir: &Path, rel: &str, items: &mut Vec<Item>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("{} kann nicht gelesen werden", dir.display()))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let meta = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = format!("{rel}/{name}");
        if meta.file_type().is_symlink() {
            continue;
        } else if meta.is_dir() {
            items.push(Item::Dir { rel: child.clone() });
            collect(&entry.path(), &child, items)?;
        } else {
            items.push(Item::File { rel: child, path: entry.path(), size: meta.len() });
        }
    }
    Ok(())
}

/// What the receiving side created at the top level, to finish or remove it.
enum Top {
    /// A folder created under a free name.
    Dir { path: PathBuf },
    /// A single file, written under a temporary name until it is complete.
    File { temp: PathBuf, name: String },
}

/// The receiving side of one transfer, writing below `dest`. Never overwrites:
/// the top-level item gets a free name. Removes what it wrote unless finished.
pub struct Incoming {
    dest: PathBuf,
    /// The name the sender used for the top-level item.
    sent_top: Option<String>,
    top: Option<Top>,
    current: Option<(File, u64)>,
    total: u64,
    written: u64,
    finished: Option<PathBuf>,
}

impl Incoming {
    pub fn new(dest: &Path) -> Result<Self> {
        if !dest.is_dir() {
            bail!("Zielordner {} nicht gefunden", dest.display());
        }
        Ok(Self {
            dest: dest.to_path_buf(),
            sent_top: None,
            top: None,
            current: None,
            total: 0,
            written: 0,
            finished: None,
        })
    }

    /// Total bytes announced by the sender.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Content bytes written so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Where the transfer ended up, once `End` was applied.
    pub fn finished(&self) -> Option<&Path> {
        self.finished.as_deref()
    }

    /// Applies one message. Returns `true` once the transfer is complete.
    pub fn apply(&mut self, msg: Transfer) -> Result<bool> {
        if self.finished.is_some() {
            bail!("Übertragung ist bereits abgeschlossen");
        }
        match msg {
            Transfer::Start { total } => self.total = total,
            Transfer::Dir { rel } => {
                self.close_file()?;
                match self.place(&rel)? {
                    Some(path) => std::fs::create_dir(&path)
                        .with_context(|| format!("{} kann nicht angelegt werden", path.display()))?,
                    None => {
                        let path = self.dest.join(free_name(&self.dest, self.sent_top.as_deref().unwrap_or_default()));
                        std::fs::create_dir(&path)
                            .with_context(|| format!("{} kann nicht angelegt werden", path.display()))?;
                        self.top = Some(Top::Dir { path });
                    }
                }
            }
            Transfer::File { rel, size } => {
                self.close_file()?;
                let path = match self.place(&rel)? {
                    Some(path) => path,
                    None => {
                        let name = self.sent_top.clone().unwrap_or_default();
                        let temp = self.dest.join(free_name(&self.dest, &format!("{name}.ctxpart")));
                        self.top = Some(Top::File { temp: temp.clone(), name });
                        temp
                    }
                };
                let file = File::options()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .with_context(|| format!("{} kann nicht geschrieben werden", path.display()))?;
                self.current = Some((file, size));
            }
            Transfer::Data(data) => {
                let Some((file, remaining)) = &mut self.current else { bail!("Daten ohne Datei") };
                if data.len() as u64 > *remaining {
                    bail!("Mehr Daten als angekündigt");
                }
                file.write_all(&data).context("Schreiben fehlgeschlagen")?;
                *remaining -= data.len() as u64;
                self.written += data.len() as u64;
            }
            Transfer::End => {
                self.close_file()?;
                let done = match self.top.take() {
                    Some(Top::Dir { path }) => path,
                    Some(Top::File { temp, name }) => {
                        let path = self.dest.join(free_name(&self.dest, &name));
                        std::fs::rename(&temp, &path).context("Datei konnte nicht umbenannt werden")?;
                        path
                    }
                    None => bail!("Leere Übertragung"),
                };
                self.finished = Some(done);
                return Ok(true);
            }
            Transfer::Failed(reason) => bail!(reason),
            Transfer::Cancel => bail!("Übertragung abgebrochen"),
        }
        Ok(false)
    }

    /// Where `rel` goes: `None` for the top-level item itself (its name is
    /// recorded), otherwise the path inside the top-level folder.
    fn place(&mut self, rel: &str) -> Result<Option<PathBuf>> {
        let (first, rest) = match rel.split_once('/') {
            Some((first, rest)) => (first, Some(rest)),
            None => (rel, None),
        };
        valid_name(first)?;
        match (&self.top, &self.sent_top, rest) {
            (None, None, None) => {
                self.sent_top = Some(first.to_string());
                Ok(None)
            }
            (Some(Top::Dir { path }), Some(top), Some(rest)) if top == first => Ok(Some(join_relative(path, rest)?)),
            _ => bail!("Unerwarteter Eintrag „{rel}“"),
        }
    }

    /// The current file must be complete before the next entry starts.
    fn close_file(&mut self) -> Result<()> {
        if let Some((file, remaining)) = self.current.take() {
            if remaining > 0 {
                bail!("Datei unvollständig übertragen");
            }
            file.sync_all().ok();
        }
        Ok(())
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        self.current = None;
        // Only what this transfer created itself, under the name it picked.
        let _ = match self.top.take() {
            Some(Top::Dir { path }) => std::fs::remove_dir_all(path),
            Some(Top::File { temp, .. }) => std::fs::remove_file(temp),
            None => Ok(()),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctxremote-{label}-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pump(from: &Path, to: &Path) -> Result<PathBuf> {
        let mut out = Outgoing::open(from)?;
        let mut inc = Incoming::new(to)?;
        while let Some(msg) = out.next_msg()? {
            if inc.apply(msg)? {
                assert_eq!(inc.written(), out.total());
                return Ok(inc.finished().unwrap().to_path_buf());
            }
        }
        bail!("kein Ende")
    }

    #[test]
    fn copies_a_tree_without_overwriting() {
        let src = temp_dir("src");
        let tree = src.join("Projekt");
        std::fs::create_dir_all(tree.join("leer")).unwrap();
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        let big: Vec<u8> = (0..(CHUNK * 2 + 17)).map(|i| (i % 251) as u8).collect();
        std::fs::write(tree.join("sub/big.bin"), &big).unwrap();
        std::fs::write(tree.join("a.txt"), "hallo").unwrap();
        std::fs::write(tree.join("null.txt"), "").unwrap();

        let dst = temp_dir("dst");
        let first = pump(&tree, &dst).unwrap();
        assert_eq!(first, dst.join("Projekt"));
        assert_eq!(std::fs::read(first.join("sub/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read_to_string(first.join("a.txt")).unwrap(), "hallo");
        assert!(first.join("leer").is_dir());
        assert!(first.join("null.txt").is_file());

        let second = pump(&tree, &dst).unwrap();
        assert_eq!(second, dst.join("Projekt (2)"));

        let file = pump(&tree.join("a.txt"), &dst).unwrap();
        assert_eq!(file, dst.join("a.txt"));
        assert_eq!(pump(&tree.join("a.txt"), &dst).unwrap(), dst.join("a (2).txt"));

        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn rejects_escapes_and_cleans_up() {
        let dst = temp_dir("evil");
        let mut inc = Incoming::new(&dst).unwrap();
        inc.apply(Transfer::Start { total: 3 }).unwrap();
        inc.apply(Transfer::Dir { rel: "x".into() }).unwrap();
        assert!(inc.apply(Transfer::File { rel: "x/../../owned".into(), size: 3 }).is_err());
        assert!(inc.apply(Transfer::File { rel: "y/z".into(), size: 3 }).is_err());
        assert!(inc.apply(Transfer::Dir { rel: "other".into() }).is_err());
        drop(inc);
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0, "partial transfer is removed");

        let mut inc = Incoming::new(&dst).unwrap();
        inc.apply(Transfer::File { rel: "f".into(), size: 2 }).unwrap();
        assert!(inc.apply(Transfer::Data(vec![1, 2, 3])).is_err());
        assert!(inc.apply(Transfer::File { rel: "f/g".into(), size: 0 }).is_err());
        drop(inc);
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);

        let mut inc = Incoming::new(&dst).unwrap();
        inc.apply(Transfer::File { rel: "/etc/passwd".into(), size: 0 }).unwrap_err();
        inc.apply(Transfer::File { rel: "..".into(), size: 0 }).unwrap_err();
        drop(inc);
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn incomplete_file_is_an_error() {
        let dst = temp_dir("short");
        let mut inc = Incoming::new(&dst).unwrap();
        inc.apply(Transfer::File { rel: "f".into(), size: 5 }).unwrap();
        inc.apply(Transfer::Data(vec![1])).unwrap();
        assert!(inc.apply(Transfer::End).is_err());
        drop(inc);
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
        std::fs::remove_dir_all(&dst).unwrap();
    }
}
