//! Reading a file or folder tree into [`Transfer`] messages and writing them back.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::Transfer;

use super::{free_name, join_relative, valid_name, CHUNK};

/// The temporary name of a single file while it is received, with its size,
/// so an interrupted copy can be continued only by the same file.
pub fn part_name(name: &str, size: u64) -> String {
    format!("{name}.{size}.ctxpart")
}

/// Partial copies being written right now: not to be continued by a second
/// transfer of the same file, which would write into them as well.
static ACTIVE: std::sync::Mutex<std::collections::BTreeSet<PathBuf>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Whether a transfer is writing `path` right now.
pub fn part_active(path: &Path) -> bool {
    ACTIVE.lock().unwrap().contains(path)
}

/// Bytes before the point of continuing that both sides compare.
const CHECK_BYTES: u64 = 64 * 1024;

/// A fingerprint of the bytes of `path` just before `len` (up to 64 KiB):
/// a continued copy must match its source there, or it starts over. Catches
/// another file of the same name and size, and one rewritten in between.
pub fn tail_check(path: &Path, len: u64) -> Result<u64> {
    use sha2::Digest;
    let mut file = File::open(path)?;
    let start = len.saturating_sub(CHECK_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0; (len - start) as usize];
    file.read_exact(&mut buf)?;
    let mut hash = sha2::Sha256::new();
    hash.update(len.to_le_bytes());
    hash.update(&buf);
    Ok(u64::from_le_bytes(hash.finalize()[..8].try_into().expect("8 bytes")))
}

/// Finds an interrupted copy of `name` in `dir`: its total size and the bytes
/// already there. One that a running transfer writes does not count.
pub fn find_part(dir: &Path, name: &str) -> Option<(u64, u64)> {
    let prefix = format!("{name}.");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        if part_active(&entry.path()) {
            return None;
        }
        let file = entry.file_name().to_string_lossy().into_owned();
        let size: u64 = file.strip_prefix(&prefix)?.strip_suffix(".ctxpart")?.parse().ok()?;
        let have = entry.metadata().ok().filter(|m| m.is_file())?.len();
        (have > 0 && have < size).then_some((size, have))
    })
}

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
    /// Bytes of the single file the receiver already has.
    skip: u64,
}

impl Outgoing {
    /// Like [`Outgoing::open`], but a single file continues at `offset` when
    /// it is `size` bytes long and its bytes before `offset` match `check`
    /// ([`tail_check`]), as the receiver's interrupted copy was.
    /// Returns where it continues: `offset`, or 0 if the file differs.
    pub fn open_from(path: &Path, offset: u64, size: u64, check: u64) -> Result<(Self, u64)> {
        let mut out = Self::open(path)?;
        let single = matches!(out.items.as_slice(), [Item::File { size: s, .. }] if *s == size);
        if single && offset > 0 && offset < size && tail_check(path, offset).ok() == Some(check) {
            out.skip = offset;
            out.total -= offset;
            Ok((out, offset))
        } else {
            Ok((out, 0))
        }
    }

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
        Ok(Self { items: items.into_iter(), total, started: false, ended: false, current: None, buffer: Vec::new(), skip: 0 })
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
                let mut file = File::open(&path).with_context(|| format!("{} kann nicht gelesen werden", path.display()))?;
                // Only a single file is ever continued, so `skip` is its own.
                let skip = std::mem::take(&mut self.skip);
                if skip > 0 {
                    file.seek(SeekFrom::Start(skip))?;
                }
                self.current = Some((file, size - skip));
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
    /// Bytes of an interrupted copy to continue (see [`Incoming::resume_at`]).
    resume: u64,
    /// Keep a single file's partial copy when dropped (connection lost).
    keep: bool,
}

impl Incoming {
    /// Continues the interrupted copy of the single file that follows, which
    /// has `offset` bytes; the sender starts there.
    pub fn resume_at(&mut self, offset: u64) {
        self.resume = offset;
    }

    /// Dropped from now on, a single file's partial copy stays for a later
    /// [`Incoming::resume_at`]. For a lost connection, not for a cancel.
    pub fn keep_partial(&mut self) {
        self.keep = true;
    }

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
            resume: 0,
            keep: false,
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
                let (path, resume) = match self.place(&rel)? {
                    Some(path) => (path, 0),
                    None => {
                        let name = self.sent_top.clone().unwrap_or_default();
                        let part = part_name(&name, size);
                        let resume = std::mem::take(&mut self.resume);
                        // A leftover copy that is not continued keeps its name.
                        let temp = if resume > 0 { self.dest.join(part) } else { self.dest.join(free_name(&self.dest, &part)) };
                        (temp, resume)
                    }
                };
                let file = if resume > 0 {
                    let file = File::options()
                        .append(true)
                        .open(&path)
                        .with_context(|| format!("{} kann nicht fortgesetzt werden", path.display()))?;
                    if file.metadata()?.len() != resume || resume >= size {
                        bail!("Die angefangene Datei passt nicht mehr");
                    }
                    file
                } else {
                    File::options()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .with_context(|| format!("{} kann nicht geschrieben werden", path.display()))?
                };
                if self.top.is_none() {
                    ACTIVE.lock().unwrap().insert(path.clone());
                    self.top = Some(Top::File { temp: path.clone(), name: self.sent_top.clone().unwrap_or_default() });
                }
                self.current = Some((file, size - resume));
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
                        ACTIVE.lock().unwrap().remove(&temp);
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
        if let Some(Top::File { temp, .. }) = &self.top {
            ACTIVE.lock().unwrap().remove(temp);
        }
        // Only what this transfer created itself, under the name it picked.
        let _ = match self.top.take() {
            Some(Top::Dir { path }) => std::fs::remove_dir_all(path),
            // Something to continue later, if the connection was lost.
            Some(Top::File { temp, .. }) if self.keep && std::fs::metadata(&temp).is_ok_and(|m| m.len() > 0) => Ok(()),
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
    fn an_interrupted_file_continues() {
        let src = temp_dir("resume-src");
        let data: Vec<u8> = (0..(CHUNK * 3 + 5)).map(|i| (i % 253) as u8).collect();
        let file = src.join("gross.bin");
        std::fs::write(&file, &data).unwrap();
        let size = data.len() as u64;
        let dst = temp_dir("resume-dst");

        // The connection drops after the first chunk.
        let mut out = Outgoing::open(&file).unwrap();
        let mut inc = Incoming::new(&dst).unwrap();
        for _ in 0..3 {
            inc.apply(out.next_msg().unwrap().unwrap()).unwrap();
        }
        inc.keep_partial();
        drop(inc);
        let (part_size, have) = find_part(&dst, "gross.bin").expect("Teil bleibt liegen");
        assert_eq!((part_size, have), (size, CHUNK as u64));

        // While a transfer writes a piece, nobody else continues it.
        let part = dst.join(part_name("gross.bin", size));
        let mut busy = Incoming::new(&dst).unwrap();
        busy.resume_at(have);
        let mut probe = Outgoing::open_from(&file, have, part_size, tail_check(&part, have).unwrap()).unwrap().0;
        busy.apply(probe.next_msg().unwrap().unwrap()).unwrap();
        busy.apply(probe.next_msg().unwrap().unwrap()).unwrap();
        assert!(find_part(&dst, "gross.bin").is_none(), "wird gerade geschrieben");
        busy.keep_partial();
        drop(busy);
        assert_eq!(find_part(&dst, "gross.bin"), Some((size, have)));

        // Another file of the same name and size does not continue it.
        let other = src.join("anders.bin");
        let mut changed = data.clone();
        changed[CHUNK - 1] ^= 0xff;
        std::fs::write(&other, &changed).unwrap();
        let check = tail_check(&part, have).unwrap();
        assert_eq!(Outgoing::open_from(&other, have, part_size, check).unwrap().1, 0);

        // Continued where it stopped; only the rest travels.
        let (mut out, offset) = Outgoing::open_from(&file, have, part_size, check).unwrap();
        assert_eq!(offset, have);
        assert_eq!(out.total(), size - have);
        let mut inc = Incoming::new(&dst).unwrap();
        inc.resume_at(offset);
        let mut sent = 0;
        while let Some(msg) = out.next_msg().unwrap() {
            if let Transfer::Data(d) = &msg {
                sent += d.len() as u64;
            }
            if inc.apply(msg).unwrap() {
                break;
            }
        }
        assert_eq!(sent, size - have);
        assert_eq!(std::fs::read(dst.join("gross.bin")).unwrap(), data);
        assert!(find_part(&dst, "gross.bin").is_none());

        // A changed file starts over.
        let (_, offset) = Outgoing::open_from(&file, 10, size + 1, 0).unwrap();
        assert_eq!(offset, 0);
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn a_cancelled_file_leaves_nothing() {
        let dst = temp_dir("cancel");
        let mut inc = Incoming::new(&dst).unwrap();
        inc.apply(Transfer::File { rel: "f".into(), size: 5 }).unwrap();
        inc.apply(Transfer::Data(vec![1, 2])).unwrap();
        drop(inc);
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
        // A wrong offset is refused instead of corrupting the file.
        let mut inc = Incoming::new(&dst).unwrap();
        inc.apply(Transfer::File { rel: "g".into(), size: 5 }).unwrap();
        inc.apply(Transfer::Data(vec![1, 2])).unwrap();
        inc.keep_partial();
        drop(inc);
        let mut inc = Incoming::new(&dst).unwrap();
        inc.resume_at(3);
        assert!(inc.apply(Transfer::File { rel: "g".into(), size: 5 }).is_err());
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
