//! Screen capture backends.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{displays, Capturer};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::display_geometry;
#[cfg(target_os = "macos")]
pub use self::macos::{displays, screen_permission, Capturer};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "macos")))]
pub use self::unsupported::{displays, Capturer};

/// Whether this platform can be controlled remotely (it can always view).
pub const HOST_SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

#[cfg(not(any(windows, target_os = "macos")))]
const UNSUPPORTED: &str = "Dieses Betriebssystem kann noch nicht ferngesteuert werden";

/// A monitor in virtual-desktop coordinates (physical pixels).
#[derive(Debug, Clone)]
pub struct Display {
    pub index: u8,
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl From<&Display> for ctxremote_proto::session::DisplayInfo {
    fn from(d: &Display) -> Self {
        Self {
            index: d.index,
            name: d.name.clone(),
            width: d.width,
            height: d.height,
            primary: d.primary,
        }
    }
}

/// How a pointer image from the system is laid out (Windows' three formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerFormat {
    /// 1 bit per pixel: an AND mask, then an XOR mask of the same size below it.
    Monochrome,
    /// 32-bit BGRA with real alpha.
    Color,
    /// 32-bit BGR; the alpha byte is a mask: 0 = draw the colour, 0xFF = XOR with the screen.
    MaskedColor,
}

/// Converts a pointer image to straight RGBA. `height` is the visible height
/// (half the mask height for monochrome). A browser cursor cannot invert the
/// screen, so inverting pixels become a half-transparent dark.
pub fn pointer_rgba(format: PointerFormat, width: u32, height: u32, pitch: usize, data: &[u8]) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    match format {
        PointerFormat::Monochrome => {
            if data.len() < pitch * h * 2 {
                return None;
            }
            let bit = |row: usize, x: usize| data[row * pitch + x / 8] & (0x80 >> (x % 8)) != 0;
            for y in 0..h {
                for x in 0..w {
                    let (and, xor) = (bit(y, x), bit(y + h, x));
                    let px = match (and, xor) {
                        (false, false) => [0, 0, 0, 255],
                        (false, true) => [255, 255, 255, 255],
                        (true, false) => [0, 0, 0, 0],
                        // Inverts the screen; dark and half transparent reads well on most backgrounds.
                        (true, true) => [0, 0, 0, 160],
                    };
                    out[(y * w + x) * 4..][..4].copy_from_slice(&px);
                }
            }
        }
        PointerFormat::Color | PointerFormat::MaskedColor => {
            if data.len() < pitch * h || pitch < w * 4 {
                return None;
            }
            for y in 0..h {
                for x in 0..w {
                    let src = &data[y * pitch + x * 4..][..4];
                    let (b, g, r, a) = (src[0], src[1], src[2], src[3]);
                    let px = match format {
                        PointerFormat::Color => [r, g, b, a],
                        _ if a == 0 => [r, g, b, 255],
                        // XOR with black leaves the screen as is.
                        _ if (r, g, b) == (0, 0, 0) => [0, 0, 0, 0],
                        _ => [0, 0, 0, 160],
                    };
                    out[(y * w + x) * 4..][..4].copy_from_slice(&px);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_pointer() {
        // 8×1: AND row then XOR row, one byte each.
        let data = [0b1100_0000, 0b0100_0001];
        let rgba = pointer_rgba(PointerFormat::Monochrome, 8, 1, 1, &data).unwrap();
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0], "AND 1, XOR 0: transparent");
        assert_eq!(&rgba[4..8], &[0, 0, 0, 160], "AND 1, XOR 1: inverted");
        assert_eq!(&rgba[8..12], &[0, 0, 0, 255], "AND 0, XOR 0: black");
        assert_eq!(&rgba[28..32], &[255, 255, 255, 255], "AND 0, XOR 1: white");
    }

    #[test]
    fn color_pointers() {
        let data = [10, 20, 30, 40, 1, 2, 3, 0];
        assert_eq!(pointer_rgba(PointerFormat::Color, 2, 1, 8, &data).unwrap(), [30, 20, 10, 40, 3, 2, 1, 0]);
        assert_eq!(
            pointer_rgba(PointerFormat::MaskedColor, 2, 1, 8, &data).unwrap(),
            [0, 0, 0, 160, 3, 2, 1, 255]
        );
        assert!(pointer_rgba(PointerFormat::Color, 4, 4, 16, &data).is_none());
    }
}
