//! Client updates end to end: `update-sign` publishes into the data folder,
//! the server announces and delivers, the client verifies.

use std::process::{Child, Command};
use std::time::Duration;

use ctxremote_core::update::{check_with, download};

const SECRET: &str = "0707070707070707070707070707070707070707070707070707070707070707";

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn public_key() -> [u8; 32] {
    let secret: [u8; 32] = hex::decode(SECRET).unwrap().try_into().unwrap();
    ed25519_dalek::SigningKey::from_bytes(&secret).verifying_key().to_bytes()
}

fn publish(data: &std::path::Path, version: &str, installer: &[u8], secret: &str) {
    let file = data.join(format!("setup-{version}.exe"));
    std::fs::write(&file, installer).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_ctxremote-server"))
        .args(["update-sign", "--platform", "windows-x86_64", "--version", version, "--notes", "Test"])
        .arg("--file")
        .arg(&file)
        .arg("--out")
        .arg(data.join("updates"))
        .env("CTXREMOTE_UPDATE_SIGNING_KEY", secret)
        .status()
        .unwrap();
    assert!(status.success());
}

async fn start(data: &std::path::Path) -> (Server, String) {
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_ctxremote-server"))
        .args(["--listen", &format!("127.0.0.1:{port}"), "--data"])
        .arg(data)
        .spawn()
        .unwrap();
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (Server(child), addr)
}

#[tokio::test(flavor = "multi_thread")]
async fn signed_update_is_offered_and_delivered() {
    let data = std::env::temp_dir().join(format!("ctxremote-updates-{}", free_port()));
    std::fs::create_dir_all(&data).unwrap();
    let (_server, addr) = start(&data).await;
    let key = public_key();

    // Nothing published yet.
    assert!(check_with(&addr, "windows-x86_64", "0.1.0", &key).await.unwrap().is_none());

    // Larger than one chunk, so the download spans several frames.
    let installer: Vec<u8> = (0..(2 * 1024 * 1024 + 99)).map(|i| (i % 251) as u8).collect();
    publish(&data, "0.1.5", &installer, SECRET);

    let info = check_with(&addr, "windows-x86_64", "0.1.0", &key).await.unwrap().expect("update offered");
    assert_eq!(info.version, "0.1.5");
    assert_eq!(info.notes, "Test");
    // Not newer, other platform: nothing.
    assert!(check_with(&addr, "windows-x86_64", "0.1.5", &key).await.unwrap().is_none());
    assert!(check_with(&addr, "windows-aarch64", "0.1.0", &key).await.unwrap().is_none());

    let target = data.join("download");
    let path = download(&addr, &info, &target).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), installer);

    // A different key (e.g. a compromised server signing its own build) is refused.
    let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
    assert!(check_with(&addr, "windows-x86_64", "0.1.0", &other).await.is_err());

    // The release changed meanwhile: the old download request is refused.
    publish(&data, "0.1.6", b"neu", SECRET);
    assert!(download(&addr, &info, &target).await.is_err());
    assert!(!target.join("CTXRemote-0.1.5-setup.part").exists(), "partial file removed");

    std::fs::remove_dir_all(&data).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_installer_is_rejected() {
    let data = std::env::temp_dir().join(format!("ctxremote-updates-{}", free_port()));
    std::fs::create_dir_all(&data).unwrap();
    let (_server, addr) = start(&data).await;
    publish(&data, "0.2.0", b"original installer", SECRET);
    let info = check_with(&addr, "windows-x86_64", "0.1.0", &public_key()).await.unwrap().unwrap();

    // Someone swaps the file on the server after signing.
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.join("updates/windows-x86_64.json")).unwrap()).unwrap();
    let file = manifest["file"].as_str().unwrap();
    std::fs::write(data.join("updates").join(file), b"evil installer!!!!").unwrap();

    let err = download(&addr, &info, &data.join("download")).await.unwrap_err();
    assert!(format!("{err:#}").contains("Signatur") || format!("{err:#}").contains("größer"), "{err:#}");
    std::fs::remove_dir_all(&data).unwrap();
}
