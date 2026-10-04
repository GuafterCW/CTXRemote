//! The screen side of a hosted session: capture, encoding, input and clipboard.
//!
//! It knows nothing about the network. It reads [`ViewerMsg`]s and writes
//! [`HostMsg`]s, so it can run in the same process as the host or in a separate
//! agent process behind a pipe (see `docs/WINDOWS-SERVICE.md`).

use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ctxremote_proto::session::{HostInfo, HostMsg, VideoCodec, VideoFrame, ViewerMsg};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::capture::{self, Capturer};
use crate::clipboard::ClipboardSync;
use crate::encoder::{pack_bgra, VideoEncoder};
use crate::files::service::FileService;
use crate::input::Injector;

const FPS: f32 = 30.0;

/// Serves one session until `inbox` closes, the viewer says `Bye` or `outbox`
/// is gone. Sends `Welcome` first, or `Bye` with a reason if there is no screen.
/// A small `outbox` applies backpressure to the capture loop.
pub async fn run(mut inbox: mpsc::Receiver<ViewerMsg>, outbox: mpsc::Sender<HostMsg>) -> Result<()> {
    let displays = match capture::displays() {
        Ok(displays) if !displays.is_empty() => displays,
        result => {
            let reason = result.err().map_or("Kein Bildschirm gefunden".into(), |e| format!("{e:#}"));
            // Tell the viewer why instead of just dropping the connection.
            let _ = outbox.send(HostMsg::Bye(reason.clone())).await;
            anyhow::bail!(reason);
        }
    };
    let primary = displays.iter().find(|d| d.primary).unwrap_or(&displays[0]);
    let mut active = primary.clone();
    outbox
        .send(HostMsg::Welcome(HostInfo {
            hostname: whoami::devicename(),
            username: whoami::username(),
            os: whoami::distro(),
            displays: displays.iter().map(Into::into).collect(),
            active_display: active.index,
        }))
        .await?;

    // Capture and encoding block, so they live on their own thread. The small
    // channel applies backpressure: on a slow link we capture less often
    // instead of queueing stale frames.
    let (frames_tx, mut frames_rx) = mpsc::channel::<VideoFrame>(2);
    let (commands, commands_rx) = std_mpsc::channel::<VideoCommand>();
    let first_display = active.index;
    let video = std::thread::Builder::new()
        .name("ctxremote-video".into())
        .spawn(move || {
            if let Err(e) = video_loop(first_display, commands_rx, frames_tx) {
                warn!("Videoübertragung beendet: {e:#}");
            }
        })?;

    let mut injector = Injector::new(&active);
    // The sender is kept so the receiver stays pending if there is no clipboard.
    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel::<String>();
    let clipboard = {
        let clip_tx = clip_tx.clone();
        ClipboardSync::start(false, move |text| {
            let _ = clip_tx.send(text);
        })
    };
    // Started on the first file request; most sessions never need it.
    let mut files: Option<FileService> = None;
    let result: Result<()> = async {
        loop {
            tokio::select! {
                Some(text) = clip_rx.recv() => outbox.send(HostMsg::Clipboard(text)).await?,
                frame = frames_rx.recv() => match frame {
                    Some(frame) => outbox.send(HostMsg::Video(frame)).await?,
                    None => anyhow::bail!("Bildschirmaufnahme nicht verfügbar"),
                },
                msg = inbox.recv() => match msg {
                    Some(ViewerMsg::Input(event)) => injector.apply(&event),
                    Some(ViewerMsg::RequestKeyframe) => { let _ = commands.send(VideoCommand::Keyframe); }
                    Some(ViewerMsg::SelectDisplay(index)) => {
                        if let Some(display) = displays.iter().find(|d| d.index == index) {
                            active = display.clone();
                            injector.set_display(&active);
                            let _ = commands.send(VideoCommand::Display(index));
                        }
                    }
                    Some(ViewerMsg::Clipboard(text)) => {
                        if let Some(clipboard) = &clipboard {
                            clipboard.apply(text);
                        }
                    }
                    Some(msg @ (ViewerMsg::File { .. } | ViewerMsg::Transfer { .. })) => {
                        if files.is_none() {
                            files = Some(FileService::start(outbox.clone())?);
                        }
                        files.as_ref().expect("started above").handle(msg);
                    }
                    Some(ViewerMsg::LockScreen) => {
                        if let Err(e) = crate::sas::lock() {
                            warn!("Sperren fehlgeschlagen: {e:#}");
                        }
                    }
                    // Windows accepts SendSAS only from the service itself, so the host handles it.
                    Some(ViewerMsg::SecureAttention) | Some(ViewerMsg::Hello { .. }) => {}
                    Some(ViewerMsg::Bye) | None => return Ok(()),
                },
            }
        }
    }
    .await;

    injector.release_all();
    drop(files);
    drop(clipboard);
    drop(clip_tx);
    drop(commands);
    drop(frames_rx);
    let _ = tokio::task::spawn_blocking(move || video.join()).await;
    result
}

enum VideoCommand {
    Keyframe,
    Display(u8),
}

fn video_loop(
    mut display: u8,
    commands: std_mpsc::Receiver<VideoCommand>,
    frames: mpsc::Sender<VideoFrame>,
) -> Result<()> {
    let frame_time = Duration::from_secs_f32(1.0 / FPS);
    let started = Instant::now();
    'display: loop {
        let mut capturer = match Capturer::new(display) {
            Ok(capturer) => capturer,
            Err(e) => {
                // E.g. during a session switch; the session survives and capture resumes.
                debug!("Bildschirmaufnahme nicht bereit: {e:#}");
                if pause(&commands, &mut display) {
                    return Ok(());
                }
                continue 'display;
            }
        };
        let mut encoder: Option<VideoEncoder> = None;
        let mut packed = Vec::new();
        let mut have_frame = false;
        let mut force_key = true;
        loop {
            let tick = Instant::now();
            loop {
                match commands.try_recv() {
                    Ok(VideoCommand::Keyframe) => force_key = true,
                    Ok(VideoCommand::Display(index)) if index != display => {
                        display = index;
                        continue 'display;
                    }
                    Ok(VideoCommand::Display(_)) => {}
                    Err(std_mpsc::TryRecvError::Empty) => break,
                    Err(std_mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }

            let changed = match capturer.next_frame(frame_time.as_millis() as u32, |data, pitch, w, h| {
                pack_bgra(data, pitch, w, h, &mut packed);
            }) {
                Ok(changed) => changed,
                Err(e) => {
                    debug!("Bildschirmaufnahme unterbrochen: {e:#}");
                    if pause(&commands, &mut display) {
                        return Ok(());
                    }
                    continue 'display;
                }
            };
            have_frame |= changed;
            if !have_frame || !(changed || force_key) {
                continue;
            }

            let (width, height) = (capturer.display().width, capturer.display().height);
            if encoder.as_ref().map(|e| e.size()) != Some((width & !1, height & !1)) {
                encoder = Some(VideoEncoder::new(width, height, FPS)?);
            }
            let encoder = encoder.as_mut().expect("created above");
            if force_key {
                encoder.force_keyframe();
            }
            let (w, h) = encoder.size();
            let encoded = encoder.encode(&packed)?;
            if encoded.data.is_empty() {
                // Rate control skipped this frame; a pending keyframe is retried next tick.
                continue;
            }
            force_key &= !encoded.keyframe;
            let frame = VideoFrame {
                display,
                width: w,
                height: h,
                keyframe: encoded.keyframe,
                codec: VideoCodec::H264,
                timestamp_us: started.elapsed().as_micros() as u64,
                data: encoded.data.to_vec(),
            };
            if frames.blocking_send(frame).is_err() {
                return Ok(());
            }
            if let Some(rest) = frame_time.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}

/// Waits a moment before capture is retried. Applies display changes meanwhile;
/// returns `true` if the session is over.
fn pause(commands: &std_mpsc::Receiver<VideoCommand>, display: &mut u8) -> bool {
    std::thread::sleep(Duration::from_millis(500));
    loop {
        match commands.try_recv() {
            Ok(VideoCommand::Display(index)) => *display = index,
            // A fresh capturer starts with a keyframe anyway.
            Ok(VideoCommand::Keyframe) => {}
            Err(std_mpsc::TryRecvError::Empty) => return false,
            Err(std_mpsc::TryRecvError::Disconnected) => return true,
        }
    }
}
