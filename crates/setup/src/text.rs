//! Text with the system font, rasterized by fontdue straight into the pixmap.

use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};
use tiny_skia::{Color, Pixmap};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Semibold,
}

pub struct Fonts {
    regular: Font,
    semibold: Font,
    cache: HashMap<(Weight, char, u32), (Metrics, Vec<u8>)>,
}

/// Segoe UI on Windows; elsewhere (previews while developing) whatever sans
/// serif font is around.
fn candidates(weight: Weight) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let fonts = std::path::Path::new(&windir).join("Fonts");
        let names: &[&str] = match weight {
            Weight::Regular => &["segoeui.ttf"],
            Weight::Semibold => &["seguisb.ttf", "segoeuib.ttf"],
        };
        out.extend(names.iter().map(|n| fonts.join(n)));
    } else {
        let names: &[&str] = match weight {
            Weight::Regular => &[
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ],
            Weight::Semibold => &[
                "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            ],
        };
        out.extend(names.iter().map(Into::into));
    }
    out
}

fn load(weight: Weight) -> Option<Font> {
    candidates(weight)
        .into_iter()
        .filter_map(|path| std::fs::read(path).ok())
        .find_map(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
}

impl Fonts {
    pub fn load() -> Option<Fonts> {
        Some(Fonts { regular: load(Weight::Regular)?, semibold: load(Weight::Semibold)?, cache: HashMap::new() })
    }

    fn font(&self, weight: Weight) -> &Font {
        match weight {
            Weight::Regular => &self.regular,
            Weight::Semibold => &self.semibold,
        }
    }

    /// Width of `text` in pixels at `size` pixels.
    pub fn width(&self, weight: Weight, size: f32, text: &str) -> f32 {
        let font = self.font(weight);
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous {
                width += font.horizontal_kern(p, c, size).unwrap_or(0.0);
            }
            width += font.metrics(c, size).advance_width;
            previous = Some(c);
        }
        width
    }

    /// Splits `text` into lines no wider than `max` pixels.
    pub fn wrap(&self, weight: Weight, size: f32, text: &str, max: f32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && self.width(weight, size, &candidate) > max {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    /// Draws `text` with its baseline at `y`, starting at `x` (pixels).
    pub fn draw(&mut self, pixmap: &mut Pixmap, weight: Weight, size: f32, x: f32, y: f32, color: Color, text: &str) {
        let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
        let color = color.to_color_u8();
        let (cr, cg, cb) = (color.red() as u32, color.green() as u32, color.blue() as u32);
        let pixels = pixmap.data_mut();
        let mut pen = x;
        let mut previous = None;
        for c in text.chars() {
            let font = match weight {
                Weight::Regular => &self.regular,
                Weight::Semibold => &self.semibold,
            };
            if let Some(p) = previous {
                pen += font.horizontal_kern(p, c, size).unwrap_or(0.0);
            }
            previous = Some(c);
            let key = (weight, c, size.to_bits());
            if !self.cache.contains_key(&key) {
                let glyph = font.rasterize(c, size);
                self.cache.insert(key, glyph);
            }
            let (metrics, coverage) = &self.cache[&key];
            let left = (pen + metrics.xmin as f32).round() as i32;
            let top = (y - (metrics.height as i32 + metrics.ymin) as f32).round() as i32;
            for row in 0..metrics.height as i32 {
                let py = top + row;
                if py < 0 || py >= height {
                    continue;
                }
                for col in 0..metrics.width as i32 {
                    let px = left + col;
                    if px < 0 || px >= width {
                        continue;
                    }
                    let a = coverage[(row * metrics.width as i32 + col) as usize] as u32;
                    if a == 0 {
                        continue;
                    }
                    // The background is opaque, so plain blending is enough.
                    let i = ((py * width + px) * 4) as usize;
                    let blend = |dst: u8, src: u32| ((dst as u32 * (255 - a) + src * a + 127) / 255) as u8;
                    pixels[i] = blend(pixels[i], cr);
                    pixels[i + 1] = blend(pixels[i + 1], cg);
                    pixels[i + 2] = blend(pixels[i + 2], cb);
                }
            }
            pen += metrics.advance_width;
        }
    }
}
