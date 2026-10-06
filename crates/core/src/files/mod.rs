//! File transfer and the remote file browser.
//!
//! The same code runs on both ends: [`list`] and friends serve the host's
//! browser and the viewer's local pane, [`Outgoing`] reads a file or folder
//! tree into [`Transfer`] messages and [`Incoming`] writes them back to disk.
//! [`service`] is the host side, [`client`] the viewer side. See
//! `docs/FILE-TRANSFER.md`.

pub mod client;
pub mod service;
mod stream;
mod user;

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{bail, Context, Result};
use ctxremote_proto::session::{EntryKind, FileEntry, Listing};

pub use stream::{find_part, part_name, Incoming, Outgoing};
pub use user::UserContext;

/// Bytes per `Transfer::Data` message: small enough not to hold up video and
/// input on the shared connection for long.
pub const CHUNK: usize = 64 * 1024;

/// Upload bytes the viewer may send before the host acknowledges them. Input
/// queues behind them, so this bounds the extra input lag during an upload;
/// throughput is at most this much per round trip (20 MB/s at 50 ms).
pub const WINDOW: u64 = 1024 * 1024;

/// Lists `path`, or the top level (drives and the user's folders) for "".
pub fn list(path: &str, user: &UserContext) -> Result<Listing> {
    if path.is_empty() {
        return Ok(Listing { path: String::new(), parent: None, entries: top_level(user) });
    }
    let dir = absolute(path)?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("{} kann nicht geöffnet werden", dir.display()))? {
        let Ok(entry) = entry else { continue };
        // Links are shown as what they point to; unreadable entries are skipped.
        let Ok(meta) = std::fs::metadata(entry.path()) else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(FileEntry {
            path: entry.path().to_string_lossy().into_owned(),
            name,
            kind: if meta.is_dir() { EntryKind::Dir } else { EntryKind::File },
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: modified(&meta),
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Dir)
            .cmp(&(b.kind != EntryKind::Dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let parent = Some(dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    Ok(Listing { path: dir.to_string_lossy().into_owned(), parent, entries })
}

/// Files pasted through the clipboard stay this long, so a paste can still
/// copy them; then a later paste clears them away.
const PASTE_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// A new, empty folder below `root` for one paste; clears out old ones.
pub fn paste_dir(root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > PASTE_KEEP));
            if old {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let dir = root.join(free_name(root, &stamp.to_string()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Everything directly in `dir`, for the clipboard.
pub fn entries_of(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
    paths.sort();
    if paths.is_empty() {
        anyhow::bail!("Es sind keine Dateien zum Einfügen da");
    }
    Ok(paths)
}

pub fn create_dir(path: &str) -> Result<()> {
    let path = absolute(path)?;
    std::fs::create_dir(&path).with_context(|| format!("{} kann nicht angelegt werden", path.display()))
}

pub fn rename(path: &str, name: &str) -> Result<()> {
    let from = absolute(path)?;
    let name = valid_name(name)?;
    let to = from.parent().context("Ein Laufwerk kann nicht umbenannt werden")?.join(name);
    if std::fs::symlink_metadata(&to).is_ok() {
        bail!("„{name}“ gibt es dort schon");
    }
    std::fs::rename(&from, &to).with_context(|| format!("{} kann nicht umbenannt werden", from.display()))
}

pub fn delete(paths: &[String]) -> Result<()> {
    for path in paths {
        let path = absolute(path)?;
        if path.parent().is_none() {
            bail!("Ein Laufwerk kann nicht gelöscht werden");
        }
        // Not following links: deleting a link never touches its target.
        let meta = std::fs::symlink_metadata(&path).with_context(|| format!("{} nicht gefunden", path.display()))?;
        let result = if meta.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        result.with_context(|| format!("{} kann nicht gelöscht werden", path.display()))?;
    }
    Ok(())
}

/// Drives first, then the user's well-known folders that exist.
fn top_level(user: &UserContext) -> Vec<FileEntry> {
    let mut entries: Vec<FileEntry> = user
        .places()
        .into_iter()
        .filter(|(_, path)| path.is_dir())
        .map(|(name, path)| FileEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            kind: EntryKind::Place,
            size: 0,
            modified: 0,
        })
        .collect();
    entries.extend(drives().into_iter().map(|root| FileEntry {
        name: root.clone(),
        path: root,
        kind: EntryKind::Drive,
        size: 0,
        modified: 0,
    }));
    entries
}

#[cfg(windows)]
fn drives() -> Vec<String> {
    // SAFETY: no arguments; returns a bit mask of drive letters.
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| format!("{}:\\", (b'A' + bit) as char))
        // Empty card readers and optical drives cannot be listed.
        .filter(|root| Path::new(root).is_dir())
        .collect()
}

#[cfg(not(windows))]
fn drives() -> Vec<String> {
    vec!["/".into()]
}

fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// The peer always names absolute paths; anything else is a mistake.
fn absolute(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        bail!("Ungültiger Pfad: {}", path.display());
    }
    Ok(path)
}

/// A single file or folder name, as typed by the user or sent by the peer.
pub fn valid_name(name: &str) -> Result<&str> {
    const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." || name.contains(FORBIDDEN) || name.chars().any(char::is_control) {
        bail!("Ungültiger Name: „{name}“");
    }
    Ok(name)
}

/// Turns a relative transfer path (`a/b/c`) into a path below `base`. Each part
/// must be a plain name, so a peer can never write outside `base`.
pub fn join_relative(base: &Path, rel: &str) -> Result<PathBuf> {
    let mut path = base.to_path_buf();
    let mut parts = 0;
    for part in rel.split('/') {
        valid_name(part)?;
        // Belt and braces: the platform must see the part as a plain name, too.
        let mut components = Path::new(part).components();
        if !matches!((components.next(), components.next()), (Some(Component::Normal(_)), None)) {
            bail!("Ungültiger Name: „{part}“");
        }
        path.push(part);
        parts += 1;
    }
    if parts == 0 {
        bail!("Leerer Pfad");
    }
    Ok(path)
}

/// `name`, or `name (2)`, `name (3)`, … with the extension kept, whichever is free in `dir`.
pub fn free_name(dir: &Path, name: &str) -> String {
    let taken = |n: &str| std::fs::symlink_metadata(dir.join(n)).is_ok();
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !taken(candidate))
        .expect("some number is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_inside() {
        let base = Path::new("/base");
        assert_eq!(join_relative(base, "a/b.txt").unwrap(), Path::new("/base/a/b.txt"));
        for bad in ["", "..", "a/../b", "/etc", "a//b", "C:x", "a\\b", ".", "a/./b", "x\0y"] {
            assert!(join_relative(base, bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn free_names_keep_the_extension() {
        let dir = std::env::temp_dir().join(format!("ctxremote-free-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_name(&dir, "a.txt"), "a.txt");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("a (2).txt"), "").unwrap();
        assert_eq!(free_name(&dir, "a.txt"), "a (3).txt");
        std::fs::create_dir(dir.join(".hidden")).unwrap();
        assert_eq!(free_name(&dir, ".hidden"), ".hidden (2)");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_sorts_folders_first() {
        let dir = std::env::temp_dir().join(format!("ctxremote-list-{}", rand::random::<u32>()));
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::write(dir.join("alpha.txt"), "hello").unwrap();
        let listing = list(dir.to_str().unwrap(), &UserContext::current()).unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| (e.name.as_str(), e.kind, e.size)).collect();
        assert_eq!(names, [("Zeta", EntryKind::Dir, 0), ("alpha.txt", EntryKind::File, 5)]);
        assert_eq!(listing.parent.as_deref(), dir.parent().map(|p| p.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn top_level_has_a_root() {
        let listing = list("", &UserContext::current()).unwrap();
        assert!(listing.parent.is_none());
        assert!(listing.entries.iter().any(|e| e.kind == EntryKind::Drive));
    }
}
