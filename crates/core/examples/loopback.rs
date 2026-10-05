//! End-to-end check against a running `ctxremote-server`: registers a host,
//! connects a viewer to it, streams this machine's screen for a few seconds
//! and decodes the first keyframe.
//!
//! ```text
//! cargo run -p ctxremote-server &
//! cargo run -p ctxremote-core --example loopback -- 127.0.0.1:21300
//! ```
//!
//! With `--agent-process` the screen side runs in a child process behind a
//! named pipe, the way the Windows service runs it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use ctxremote_core::config::Config;
use ctxremote_core::host::{Host, Presence};
use ctxremote_core::viewer::{ViewerEvent, ViewerSession};
use ctxremote_core::proto::DeviceId;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--agent") {
        // Started by `--agent-process` below.
        return ctxremote_core::agent_process::serve(&args[1]).await;
    }
    let agent_process = args.iter().any(|a| a == "--agent-process");
    args.retain(|a| a != "--agent-process");
    let server = args.first().cloned().unwrap_or_else(|| "127.0.0.1:21300".into());
    let config_path = std::env::temp_dir().join("ctxremote-loopback.json");
    std::env::set_var("CTXREMOTE_CONFIG", &config_path);
    let config = Config { server: server.clone(), ..Config::default() };

    let config = Arc::new(RwLock::new(config));
    let host = if agent_process {
        // The screen side runs in a child process behind a pipe, as under the service.
        let exe = std::env::current_exe()?;
        Host::start_with(config, Arc::new(ctxremote_core::agent_process::AgentProcess::new(exe)))
    } else {
        Host::start(config)
    };
    let mut presence = host.presence();
    let id: DeviceId = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Presence::Online { id } = presence.borrow_and_update().clone() {
                return id.parse().unwrap();
            }
            presence.changed().await.unwrap();
        }
    })
    .await?;
    println!("Host online als {id}");

    match ViewerSession::connect(&server, None, id, "falsch", None, |_| {}).await {
        Ok(_) => bail!("falsches Passwort wurde akzeptiert"),
        Err(e) => println!("Falsches Passwort abgewiesen: {e}"),
    }

    let frames = Arc::new(AtomicUsize::new(0));
    let bytes = Arc::new(AtomicUsize::new(0));
    let first_key = Arc::new(Mutex::new(None));
    let started = Instant::now();
    let session = {
        let (frames, bytes, first_key) = (frames.clone(), bytes.clone(), first_key.clone());
        ViewerSession::connect(&server, None, id, &host.password(), None, move |event| {
            if let ViewerEvent::Video(frame) = event {
                frames.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(frame.data.len(), Ordering::Relaxed);
                let mut first = first_key.lock().unwrap();
                if first.is_none() && frame.keyframe {
                    *first = Some((started.elapsed(), frame));
                }
            }
        })
        .await?
    };
    println!("Sitzung aufgebaut nach {:?}", started.elapsed());
    println!(
        "Verbunden mit {} ({} Bildschirm(e), {})",
        session.host.hostname,
        session.host.displays.len(),
        session.host.os
    );

    tokio::time::sleep(Duration::from_secs(4)).await;
    let Some((latency, key)) = first_key.lock().unwrap().take() else {
        bail!("kein Keyframe empfangen");
    };
    let mut decoder = openh264::decoder::Decoder::new()?;
    let decoded = decoder.decode(&key.data)?.map(|yuv| {
        use openh264::formats::YUVSource;
        yuv.dimensions()
    });
    println!(
        "Erster Keyframe nach {latency:?}: {}x{}, {} KB, dekodiert: {decoded:?}",
        key.width,
        key.height,
        key.data.len() / 1024
    );
    let n = frames.load(Ordering::Relaxed);
    println!("{n} Frames in 4 s, {} KB gesamt", bytes.load(Ordering::Relaxed) / 1024);
    if decoded.is_none() {
        bail!("Keyframe ließ sich nicht dekodieren");
    }
    drop(session);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // With approval required, a refusal ends the session despite the right password.
    host.require_approval(Arc::new(|peer, _profile| {
        println!("Anfrage von {peer}, wird abgelehnt");
        Box::pin(async { false })
    }));
    match ViewerSession::connect(&server, None, id, &host.password(), None, |_| {}).await {
        Ok(_) => bail!("abgelehnte Sitzung wurde trotzdem eröffnet"),
        Err(e) => println!("Ohne Zustimmung abgewiesen: {e}"),
    }
    let _ = std::fs::remove_file(config_path);
    println!("OK");
    Ok(())
}
