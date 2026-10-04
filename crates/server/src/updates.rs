//! Client releases in `<data>/updates`: one `<platform>.json` manifest per
//! platform next to the installer it names. The release pipeline writes them
//! with `update-sign`; the server only reads them, on every request, so a new
//! release needs no restart.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport};
use ctxremote_proto::rendezvous::{ServerError, ServerMsg};
use ctxremote_proto::update::{UpdateInfo, Version, CHUNK};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

/// The manifest: what clients are told, plus the installer's file name.
#[derive(Serialize, Deserialize)]
struct Manifest {
    file: String,
    info: UpdateInfo,
}

pub struct Store {
    dir: PathBuf,
}

/// Platform names become file names, so only a plain set of characters passes.
fn valid_platform(platform: &str) -> bool {
    !platform.is_empty()
        && platform.len() <= 40
        && platform.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn manifest(&self, platform: &str) -> Option<Manifest> {
        if !valid_platform(platform) {
            return None;
        }
        let json = std::fs::read_to_string(self.dir.join(format!("{platform}.json"))).ok()?;
        match serde_json::from_str::<Manifest>(&json) {
            Ok(manifest) if valid_file_name(&manifest.file) => Some(manifest),
            Ok(_) => {
                warn!(platform, "Update-Manifest nennt eine ungültige Datei");
                None
            }
            Err(e) => {
                warn!(platform, "Update-Manifest unlesbar: {e}");
                None
            }
        }
    }

    pub fn latest(&self, platform: &str) -> Option<UpdateInfo> {
        self.manifest(platform).map(|m| m.info)
    }

    /// Streams the installer in chunks, then `UpdateEnd`; `UpdateGone` if the
    /// release changed since the client checked.
    pub async fn send(&self, t: &mut Transport, platform: &str, version: &str) -> Result<()> {
        let Some(manifest) = self.manifest(platform).filter(|m| m.info.version == version) else {
            return framing::send(t, &ServerMsg::Error(ServerError::UpdateGone)).await;
        };
        let mut file = tokio::fs::File::open(self.dir.join(&manifest.file)).await?;
        let mut buffer = vec![0u8; CHUNK];
        let mut sent = 0u64;
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            sent += read as u64;
            framing::send(t, &ServerMsg::UpdateData(buffer[..read].to_vec())).await?;
        }
        framing::send(t, &ServerMsg::UpdateEnd).await?;
        info!(platform, version, bytes = sent, "Update ausgeliefert");
        Ok(())
    }
}

fn valid_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

/// Prints a fresh signing key pair for the release pipeline.
pub fn keygen() -> Result<()> {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    println!("Privater Schlüssel (GitHub-Secret CTXREMOTE_UPDATE_SIGNING_KEY, geheim halten):");
    println!("{}", hex::encode(key.to_bytes()));
    println!();
    println!("Öffentlicher Schlüssel (GitHub-Variable CTXREMOTE_UPDATE_KEY, wird in die Clients eingebaut):");
    println!("{}", hex::encode(key.verifying_key().to_bytes()));
    Ok(())
}

/// `update-sign`: copies the installer into `--out` and writes the signed
/// manifest next to it. The key comes from `CTXREMOTE_UPDATE_SIGNING_KEY` (hex),
/// so it never appears on a command line.
pub fn sign(args: Vec<String>) -> Result<()> {
    let mut platform = None;
    let mut version = None;
    let mut file = None;
    let mut out = None;
    let mut notes = String::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} braucht einen Wert"));
        match arg.as_str() {
            "--platform" => platform = Some(value()?),
            "--version" => version = Some(value()?),
            "--file" => file = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--notes" => notes = value()?,
            other => bail!("unbekanntes Argument: {other}"),
        }
    }
    let platform = platform.context("--platform fehlt")?;
    let version = version.context("--version fehlt")?;
    let file = file.context("--file fehlt")?;
    let out = out.context("--out fehlt")?;
    if !valid_platform(&platform) {
        bail!("ungültige Plattform: {platform}");
    }
    version.parse::<Version>().context("--version muss die Form 1.2.3 haben")?;

    let secret = std::env::var("CTXREMOTE_UPDATE_SIGNING_KEY").context("CTXREMOTE_UPDATE_SIGNING_KEY fehlt")?;
    let secret: [u8; 32] = hex::decode(secret.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .context("CTXREMOTE_UPDATE_SIGNING_KEY ist kein Schlüssel aus update-keygen")?;
    let key = SigningKey::from_bytes(&secret);

    let bytes = std::fs::read(&file).with_context(|| format!("{} nicht lesbar", file.display()))?;
    let sha256: [u8; 32] = Sha256::digest(&bytes).into();
    let info = UpdateInfo::sign(&key, &platform, &version, bytes.len() as u64, sha256, notes);

    std::fs::create_dir_all(&out)?;
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let name = format!("CTXRemote-{version}-{platform}.{extension}");
    write_atomic(&out.join(&name), &bytes)?;
    // The manifest last: a client never sees a release whose file is missing.
    let manifest = serde_json::to_vec_pretty(&Manifest { file: name.clone(), info })?;
    write_atomic(&out.join(format!("{platform}.json")), &manifest)?;
    println!("{name} signiert ({} Bytes), öffentlicher Schlüssel {}", bytes.len(), hex::encode(key.verifying_key().to_bytes()));
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_cannot_escape() {
        assert!(valid_platform("windows-x86_64"));
        for bad in ["", "../x", "a/b", "Windows", "a.b", "x\\y"] {
            assert!(!valid_platform(bad), "{bad}");
        }
        assert!(!valid_file_name("../secret"));
        assert!(valid_file_name("CTXRemote-0.1.2-windows-x86_64.exe"));
    }
}
