//! What the setup window shows, drawn with tiny-skia in the app's design
//! (docs/STATUS.md, "Gestaltung"): warm neutrals, petrol accent, hairlines.

use std::time::Instant;

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::text::{Fonts, Weight};

/// Window size in logical pixels.
pub const WIDTH: f32 = 520.0;
pub const HEIGHT: f32 = 340.0;
const PAD: f32 = 40.0;

pub struct Palette {
    bg: Color,
    ink: Color,
    ink2: Color,
    ink3: Color,
    line: Color,
    line_strong: Color,
    accent: Color,
    accent_hover: Color,
    accent_ink: Color,
    bad: Color,
}

fn rgb(hex: u32) -> Color {
    Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255)
}

/// The colours of app/src/styles.css.
pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: rgb(0x1a1a19),
            ink: rgb(0xedebe6),
            ink2: rgb(0xb3b1aa),
            ink3: rgb(0x85837c),
            line: rgb(0x2b2a28),
            line_strong: rgb(0x3b3a37),
            accent: rgb(0x4fb495),
            accent_hover: rgb(0x63c4a6),
            accent_ink: rgb(0x0d1a16),
            bad: rgb(0xe07a66),
        }
    } else {
        Palette {
            bg: rgb(0xfbfaf8),
            ink: rgb(0x1b1b19),
            ink2: rgb(0x54534e),
            ink3: rgb(0x8a8982),
            line: rgb(0xe2e0d9),
            line_strong: rgb(0xcbc8bf),
            accent: rgb(0x0d5c4b),
            accent_hover: rgb(0x0a4c3e),
            accent_ink: rgb(0xffffff),
            bad: rgb(0xb3412f),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Phase {
    Ready,
    Installing { started: Instant },
    Done,
    Failed(String),
}

/// Something clickable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Primary,
    Secondary,
    Close,
}

/// What the primary button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Install,
    Launch,
}

/// How this setup relates to what is already installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Existing {
    None,
    Older(String),
    Same,
    Newer(String),
}

pub struct View {
    pub phase: Phase,
    pub dark: bool,
    pub existing: Existing,
    pub version: &'static str,
    /// No installer is embedded (a development build): installing is simulated.
    pub preview: bool,
    /// A short note under the buttons, e.g. why the last attempt did not start.
    pub notice: Option<String>,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
}

impl View {
    pub fn installing(&self) -> bool {
        matches!(self.phase, Phase::Installing { .. })
    }

    /// Title, body, primary button and its action, secondary button.
    fn texts(&self) -> (String, String, Option<(&'static str, Action)>, Option<&'static str>) {
        let v = self.version;
        match &self.phase {
            Phase::Ready => match &self.existing {
                Existing::None => (
                    "CTXRemote installieren".into(),
                    "Fernzugriff auf eigene Geräte. Die Installation richtet die App und den Hintergrunddienst ein, \
                     Windows fragt dafür nach Administratorrechten."
                        .into(),
                    Some(("Installieren", Action::Install)),
                    None,
                ),
                Existing::Older(old) => (
                    "CTXRemote aktualisieren".into(),
                    format!(
                        "Auf diesem Gerät ist Version {old} installiert. Die Aktualisierung auf {v} behält alle \
                         Einstellungen und Geräte."
                    ),
                    Some(("Aktualisieren", Action::Install)),
                    None,
                ),
                Existing::Same => (
                    "CTXRemote ist installiert".into(),
                    format!(
                        "Version {v} ist bereits auf diesem Gerät. Eine erneute Installation repariert sie und \
                         behält alle Einstellungen."
                    ),
                    Some(("Erneut installieren", Action::Install)),
                    Some("CTXRemote starten"),
                ),
                Existing::Newer(newer) => (
                    "CTXRemote ist aktuell".into(),
                    format!("Auf diesem Gerät ist bereits die neuere Version {newer} installiert."),
                    Some(("CTXRemote starten", Action::Launch)),
                    Some("Schließen"),
                ),
            },
            Phase::Installing { .. } => (
                "Wird installiert …".into(),
                "Das dauert meist weniger als eine Minute. Danach finden Sie CTXRemote im Startmenü und auf dem \
                 Desktop."
                    .into(),
                None,
                None,
            ),
            Phase::Done => (
                "CTXRemote ist bereit".into(),
                "Die App und der Hintergrunddienst sind eingerichtet. Sie finden CTXRemote im Startmenü und auf dem \
                 Desktop."
                    .into(),
                Some(("CTXRemote starten", Action::Launch)),
                Some("Schließen"),
            ),
            Phase::Failed(why) => (
                "Installation fehlgeschlagen".into(),
                why.clone(),
                Some(("Erneut versuchen", Action::Install)),
                Some("Schließen"),
            ),
        }
    }

    /// What the primary button does right now, if there is one.
    pub fn primary_action(&self) -> Option<Action> {
        self.texts().2.map(|(_, action)| action)
    }

    /// What the secondary button does: launch next to "Erneut installieren",
    /// otherwise close.
    pub fn secondary_launches(&self) -> bool {
        matches!((&self.phase, &self.existing), (Phase::Ready, Existing::Same))
    }
}

/// Progress for an installer that reports none: approaches 90 % over about
/// half a minute, the rest comes when it is done.
pub fn estimated_progress(started: Instant) -> f32 {
    let t = started.elapsed().as_secs_f32();
    0.9 * (1.0 - (-t / 12.0).exp())
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("valid rounded rectangle")
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    paint
}

/// The app icon (app/app-icon.svg) at `size` logical pixels.
pub fn logo(pixmap: &mut Pixmap, x: f32, y: f32, size: f32, scale: f32) {
    let s = size / 1024.0;
    let t = Transform::from_scale(scale, scale).pre_translate(x, y).pre_scale(s, s);
    pixmap.fill_path(&rounded(0.0, 0.0, 1024.0, 1024.0, 224.0), &paint(rgb(0x16211e)), FillRule::Winding, t, None);
    let stroke = Stroke { width: 52.0, ..Stroke::default() };
    pixmap.stroke_path(&rounded(196.0, 250.0, 472.0, 372.0, 64.0), &paint(rgb(0xedebe6)), &stroke, t, None);
    pixmap.fill_path(&rounded(400.0, 430.0, 440.0, 344.0, 64.0), &paint(rgb(0x4fb495)), FillRule::Winding, t, None);
}

/// Draws the whole window; returns the clickable areas in logical pixels.
pub fn render(view: &View, fonts: &mut Fonts, pixmap: &mut Pixmap, scale: f32) -> Vec<(Hit, Rect)> {
    let p = palette(view.dark);
    let t = Transform::from_scale(scale, scale);
    pixmap.fill(p.bg);
    let mut hits = Vec::new();

    // Close button, top right; not while the installer runs.
    if !view.installing() {
        let (x, y, w, h) = (WIDTH - 46.0, 10.0, 36.0, 30.0);
        if view.hover == Some(Hit::Close) {
            pixmap.fill_path(&rounded(x, y, w, h, 6.0), &paint(p.line), FillRule::Winding, t, None);
        }
        let (cx, cy, r) = (x + w / 2.0, y + h / 2.0, 5.0);
        let mut pb = PathBuilder::new();
        pb.move_to(cx - r, cy - r);
        pb.line_to(cx + r, cy + r);
        pb.move_to(cx + r, cy - r);
        pb.line_to(cx - r, cy + r);
        let stroke = Stroke { width: 1.5, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
        pixmap.stroke_path(&pb.finish().unwrap(), &paint(p.ink2), &stroke, t, None);
        hits.push((Hit::Close, Rect::from_xywh(x, y, w, h).unwrap()));
    }

    logo(pixmap, PAD, PAD, 44.0, scale);

    let (title, body, primary, secondary) = view.texts();
    let title_y = PAD + 44.0 + 44.0;
    fonts.draw(pixmap, Weight::Semibold, 24.0 * scale, PAD * scale, title_y * scale, p.ink, &title);
    let body_color = if matches!(view.phase, Phase::Failed(_)) { p.bad } else { p.ink2 };
    let mut y = title_y + 30.0;
    for line in fonts.wrap(Weight::Regular, 14.5 * scale, &body, (WIDTH - 2.0 * PAD) * scale) {
        fonts.draw(pixmap, Weight::Regular, 14.5 * scale, PAD * scale, y * scale, body_color, &line);
        y += 22.0;
    }

    // Bottom row: a footnote on the left, buttons on the right.
    let row_h = 36.0;
    let row_y = HEIGHT - PAD - row_h;
    let text_y = row_y + row_h / 2.0 + 4.5;

    if let Phase::Installing { started } = view.phase {
        let progress = estimated_progress(started);
        let (bar_y, bar_h, bar_w) = (row_y + row_h / 2.0 - 2.0, 4.0, WIDTH - 2.0 * PAD);
        pixmap.fill_path(&rounded(PAD, bar_y, bar_w, bar_h, 2.0), &paint(p.line), FillRule::Winding, t, None);
        let fill = (bar_w * progress).max(bar_h);
        pixmap.fill_path(&rounded(PAD, bar_y, fill, bar_h, 2.0), &paint(p.accent), FillRule::Winding, t, None);
        return hits;
    }

    let button_w = |fonts: &Fonts, label: &str| fonts.width(Weight::Semibold, 14.0 * scale, label) / scale + 2.0 * 18.0;
    let mut right = WIDTH - PAD;
    if let Some((label, _)) = primary {
        let w = button_w(fonts, label);
        let x = right - w;
        let color = if view.hover == Some(Hit::Primary) { p.accent_hover } else { p.accent };
        pixmap.fill_path(&rounded(x, row_y, w, row_h, 7.0), &paint(color), FillRule::Winding, t, None);
        fonts.draw(pixmap, Weight::Semibold, 14.0 * scale, (x + 18.0) * scale, text_y * scale, p.accent_ink, label);
        hits.push((Hit::Primary, Rect::from_xywh(x, row_y, w, row_h).unwrap()));
        right = x - 10.0;
    }
    if let Some(label) = secondary {
        let w = button_w(fonts, label);
        let x = right - w;
        if view.hover == Some(Hit::Secondary) {
            pixmap.fill_path(&rounded(x, row_y, w, row_h, 7.0), &paint(p.line), FillRule::Winding, t, None);
        }
        let stroke = Stroke { width: 1.0, ..Stroke::default() };
        pixmap.stroke_path(&rounded(x + 0.5, row_y + 0.5, w - 1.0, row_h - 1.0, 7.0), &paint(p.line_strong), &stroke, t, None);
        fonts.draw(pixmap, Weight::Semibold, 14.0 * scale, (x + 18.0) * scale, text_y * scale, p.ink, label);
        hits.push((Hit::Secondary, Rect::from_xywh(x, row_y, w, row_h).unwrap()));
        right = x;
    }

    // The footnote sits left of the buttons; a notice that does not fit there
    // goes above them, the version number is simply left out.
    let (note, color) = match &view.notice {
        Some(notice) => (notice.clone(), p.bad),
        None if view.preview => (format!("Version {} · Vorschau ohne Installer", view.version), p.ink3),
        None => (format!("Version {}", view.version), p.ink3),
    };
    let fits = PAD + fonts.width(Weight::Regular, 12.5 * scale, &note) / scale + 16.0 <= right;
    let note_y = match (fits, view.notice.is_some()) {
        (true, _) => Some(text_y),
        (false, true) => Some(row_y - 16.0),
        (false, false) => None,
    };
    if let Some(y) = note_y {
        fonts.draw(pixmap, Weight::Regular, 12.5 * scale, PAD * scale, y * scale, color, &note);
    }
    hits
}
