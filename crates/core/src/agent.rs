//! The screen side of a hosted session: capture, encoding, input and clipboard.
//!
//! It knows nothing about the network. It reads [`ViewerMsg`]s and writes
//! [`HostMsg`]s, so it can run in the same process as the host or in a separate
//! agent process behind a pipe (see `docs/WINDOWS-SERVICE.md`).

use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ctxremote_proto::session::{AudioPacket, CursorShape, DrawMsg, FileOp, FileReply, HostInfo, HostMsg, Quality, VideoCodec, VideoFrame, ViewerMsg};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::capture::{self, Capturer};
use crate::clipboard::ClipboardSync;
use crate::congestion::Congestion;
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
    let (frames_tx, mut frames_rx) = mpsc::channel::<Captured>(2);
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
    let (files_tx, mut files_rx) = mpsc::unbounded_channel::<Vec<std::path::PathBuf>>();
    let (image_tx, mut image_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let clipboard = {
        let clip_tx = clip_tx.clone();
        let files_tx = files_tx.clone();
        let image_tx = image_tx.clone();
        ClipboardSync::start_with_files(
            false,
            move |text| {
                let _ = clip_tx.send(text);
            },
            Some(Box::new(move |files| {
                let _ = files_tx.send(files);
            })),
            Some(Box::new(move |png| {
                let _ = image_tx.send(png);
            })),
        )
    };
    // Started on the first file request; most sessions never need it.
    let mut files: Option<FileService> = None;
    // The host's sound, while the viewer wants it. A few packets of slack;
    // beyond that they are dropped instead of adding delay.
    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(8);
    let mut audio: Option<crate::audio::AudioCapture> = None;
    // Blanks this computer's screen while it runs (see `privacy`).
    let mut privacy: Option<crate::privacy::PrivacyMode> = None;
    // An agent that died in privacy mode may have left the pointers invisible.
    crate::privacy::restore_pointers();
    // In privacy mode capture sees an invisible pointer; the real shape is polled.
    let mut pointer_poll = tokio::time::interval(std::time::Duration::from_millis(50));
    pointer_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The viewer's lines over the shown display (see `annotate`).
    let mut drawing: Option<crate::annotate::Overlay> = None;
    // The viewer's voice on this computer's speaker (see `speaker`).
    let mut speaker: Option<crate::speaker::MicPlayer> = None;
    let result: Result<()> = async {
        loop {
            tokio::select! {
                Some(text) = clip_rx.recv() => outbox.send(HostMsg::Clipboard(text)).await?,
                Some(png) = image_rx.recv() => outbox.send(HostMsg::ClipboardImage(png)).await?,
                Some(files) = files_rx.recv() => {
                    let paths = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    outbox.send(HostMsg::ClipboardFiles(paths)).await?;
                }
                Some(data) = audio_rx.recv() => outbox.send(HostMsg::Audio(AudioPacket { data })).await?,
                _ = pointer_poll.tick(), if privacy.is_some() => {
                    if let Some(shape) = privacy.as_mut().and_then(|p| p.pointer()) {
                        outbox.send(HostMsg::Cursor(shape)).await?;
                    }
                }
                frame = frames_rx.recv() => match frame {
                    Some(Captured::Frame(frame)) => outbox.send(HostMsg::Video(frame)).await?,
                    Some(Captured::Pointer(shape)) if privacy.is_some() && crate::privacy::is_blank(&shape) => {}
                    Some(Captured::Pointer(shape)) => outbox.send(HostMsg::Cursor(shape)).await?,
                    None => anyhow::bail!("Bildschirmaufnahme nicht verfügbar"),
                },
                msg = inbox.recv() => match msg {
                    Some(ViewerMsg::Input(event)) => injector.apply(&event),
                    Some(ViewerMsg::RequestKeyframe) => { let _ = commands.send(VideoCommand::Keyframe); }
                    Some(ViewerMsg::SelectDisplay(index)) => {
                        if let Some(display) = displays.iter().find(|d| d.index == index) {
                            active = display.clone();
                            injector.set_display(&active);
                            // Lines belong to the display they were drawn on.
                            drawing = None;
                            let _ = commands.send(VideoCommand::Display(index));
                        }
                    }
                    Some(ViewerMsg::SetQuality(quality)) => { let _ = commands.send(VideoCommand::Quality(quality)); }
                    Some(ViewerMsg::SetAudio(on)) => {
                        if !on {
                            audio = None;
                        } else if audio.is_none() {
                            match crate::audio::AudioCapture::start(audio_tx.clone()) {
                                Ok(capture) => audio = Some(capture),
                                Err(e) => warn!("Kein Ton: {e:#}"),
                            }
                        }
                    }
                    // Collected on the side, so the picture keeps flowing meanwhile.
                    Some(ViewerMsg::Mic(packet)) => {
                        if speaker.is_none() {
                            match crate::speaker::MicPlayer::new() {
                                Ok(player) => speaker = Some(player),
                                Err(e) => warn!("Mikrofon der Gegenseite nicht abspielbar: {e:#}"),
                            }
                        }
                        if let Some(player) = &mut speaker {
                            player.push(&packet.data);
                        }
                    }
                    Some(ViewerMsg::Draw(DrawMsg::Clear)) => drawing = None,
                    Some(ViewerMsg::Draw(stroke)) => {
                        if let Some(line) = crate::annotate::to_line(&stroke, active.width, active.height) {
                            if drawing.is_none() {
                                match crate::annotate::Overlay::start(&active) {
                                    Ok(overlay) => drawing = Some(overlay),
                                    Err(e) => warn!("Zeichnen nicht möglich: {e:#}"),
                                }
                            }
                            if let Some(overlay) = &drawing {
                                overlay.add(line);
                            }
                        }
                    }
                    Some(ViewerMsg::GetSystemInfo) => {
                        let outbox = outbox.clone();
                        tokio::spawn(async move {
                            if let Ok(info) = tokio::task::spawn_blocking(crate::sysinfo::gather).await {
                                let _ = outbox.send(HostMsg::SystemInfo(info)).await;
                            }
                        });
                    }
                    Some(ViewerMsg::Privacy(on)) => {
                        let error = if !on {
                            privacy = None;
                            None
                        } else if privacy.is_some() {
                            None
                        } else {
                            match crate::privacy::PrivacyMode::start() {
                                Ok(mode) => {
                                    privacy = Some(mode);
                                    None
                                }
                                Err(e) => {
                                    warn!("Privatsphäre-Modus nicht möglich: {e:#}");
                                    Some(format!("{e:#}"))
                                }
                            }
                        };
                        outbox.send(HostMsg::Privacy { on: privacy.is_some(), error }).await?;
                    }
                    Some(ViewerMsg::Clipboard(text)) => {
                        if let Some(clipboard) = &clipboard {
                            clipboard.apply(text);
                        }
                    }
                    Some(ViewerMsg::ClipboardImage(png)) => {
                        if let Some(clipboard) = &clipboard {
                            clipboard.apply_image(png);
                        }
                    }
                    // Pasted files: the uploaded copies go on this computer's clipboard.
                    Some(ViewerMsg::File { req, op: FileOp::ClipboardFromDir { dir } }) => {
                        let result = match (&clipboard, crate::files::entries_of(std::path::Path::new(&dir))) {
                            (Some(clipboard), Ok(paths)) => {
                                clipboard.apply_files(paths);
                                Ok(FileReply::Done)
                            }
                            (None, _) => Err("Die Zwischenablage ist hier nicht verfügbar".to_string()),
                            (_, Err(e)) => Err(format!("{e:#}")),
                        };
                        outbox.send(HostMsg::FileReply { req, result }).await?;
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
                    Some(ViewerMsg::SecureAttention | ViewerMsg::Restart | ViewerMsg::Switch | ViewerMsg::Chat(_) | ViewerMsg::Hello { .. } | ViewerMsg::PunchAnswer { .. } | ViewerMsg::Code(_) | ViewerMsg::Recording(_) | ViewerMsg::Tunnel(_)) => {}
                    Some(ViewerMsg::Bye) | None => return Ok(()),
                },
            }
        }
    }
    .await;

    // First, so the person at the computer gets screen and input back.
    drop(privacy);
    drop(drawing);
    drop(speaker);
    injector.release_all();
    drop(audio);
    drop(files);
    drop(clipboard);
    drop(clip_tx);
    drop(files_tx);
    drop(image_tx);
    drop(commands);
    drop(frames_rx);
    let _ = tokio::task::spawn_blocking(move || video.join()).await;
    result
}

/// What the capture thread produces.
enum Captured {
    Frame(VideoFrame),
    Pointer(CursorShape),
}

enum VideoCommand {
    Keyframe,
    Display(u8),
    Quality(Quality),
}

fn video_loop(
    mut display: u8,
    commands: std_mpsc::Receiver<VideoCommand>,
    frames: mpsc::Sender<Captured>,
) -> Result<()> {
    let frame_time = Duration::from_secs_f32(1.0 / FPS);
    let started = Instant::now();
    let mut quality = Quality::default();
    // Survives display and quality changes: the link stays the same.
    let mut congestion = Congestion::new(Instant::now());
    'display: loop {
        let mut capturer = match Capturer::new(display) {
            Ok(capturer) => capturer,
            Err(e) => {
                // E.g. during a session switch; the session survives and capture resumes.
                debug!("Bildschirmaufnahme nicht bereit: {e:#}");
                if pause(&commands, &mut display, &mut quality) {
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
                    Ok(VideoCommand::Quality(q)) if q != quality => {
                        quality = q;
                        // A new encoder starts with a keyframe at the new rate.
                        encoder = None;
                    }
                    Ok(VideoCommand::Quality(_)) => {}
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
                    if pause(&commands, &mut display, &mut quality) {
                        return Ok(());
                    }
                    continue 'display;
                }
            };
            // Pointer shapes also arrive without a new image.
            if let Some(shape) = capturer.take_pointer() {
                if frames.blocking_send(Captured::Pointer(shape)).is_err() {
                    return Ok(());
                }
            }
            have_frame |= changed;
            if !have_frame || !(changed || force_key) {
                continue;
            }

            let (width, height) = (capturer.display().width, capturer.display().height);
            if encoder.as_ref().map(|e| e.size()) != Some((width & !1, height & !1)) {
                let mut fresh = VideoEncoder::new(width, height, FPS, quality)?;
                debug!(encoder = fresh.kind(), width, height, "Encoder bereit");
                fresh.set_bitrate_factor(congestion.factor());
                encoder = Some(fresh);
                force_key = true;
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
            let handing_over = Instant::now();
            if frames.blocking_send(Captured::Frame(frame)).is_err() {
                return Ok(());
            }
            // Time spent waiting here is time the connection was full.
            if let Some(factor) = congestion.record(handing_over.elapsed(), Instant::now()) {
                encoder.set_bitrate_factor(factor);
                debug!(factor, bitrate = encoder.bitrate(), "Bitrate angepasst");
            }
            if let Some(rest) = frame_time.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}

/// Waits a moment before capture is retried. Applies display changes meanwhile;
/// returns `true` if the session is over.
fn pause(commands: &std_mpsc::Receiver<VideoCommand>, display: &mut u8, quality: &mut Quality) -> bool {
    std::thread::sleep(Duration::from_millis(500));
    loop {
        match commands.try_recv() {
            Ok(VideoCommand::Display(index)) => *display = index,
            Ok(VideoCommand::Quality(q)) => *quality = q,
            // A fresh capturer starts with a keyframe anyway.
            Ok(VideoCommand::Keyframe) => {}
            Err(std_mpsc::TryRecvError::Empty) => return false,
            Err(std_mpsc::TryRecvError::Disconnected) => return true,
        }
    }
}
