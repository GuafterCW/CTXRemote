//! Embeds the NSIS installer named by CTXREMOTE_SETUP_PAYLOAD (built before by
//! `tauri build`; release.yml sets it). Without it the setup only simulates
//! installing, which is enough to look at it while developing.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=CTXREMOTE_SETUP_PAYLOAD");
    println!("cargo:rerun-if-env-changed=CTXREMOTE_VERSION");
    println!("cargo:rerun-if-changed=setup.manifest");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("payload.exe");
    match std::env::var_os("CTXREMOTE_SETUP_PAYLOAD") {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&path).display());
            std::fs::copy(&path, &out).expect("CTXREMOTE_SETUP_PAYLOAD must name the NSIS installer");
        }
        None => std::fs::write(&out, []).unwrap(),
    }
    // Icon, version info and manifest of the EXE. A type check from Linux has
    // no resource compiler, and that is fine there.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let icon = dir.join("../../app/src-tauri/icons/icon.ico");
        let manifest = dir.join("setup.manifest");
        // Absolute paths, so the resource compiler finds them from anywhere.
        let rc = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("setup.rc");
        let quoted = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            &rc,
            format!("1 ICON \"{}\"\n1 24 \"{}\"\n", quoted(icon), quoted(manifest)),
        )
        .unwrap();
        let _ = embed_resource::compile(&rc, embed_resource::NONE).manifest_optional();
    }
}
