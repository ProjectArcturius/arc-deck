//! OKLCh conversions for the v1→v2 theme migration.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! Ported from OpenCode v2.0.27 `packages/theme/src/tui/color.ts` and the byte/ANSI helpers
//! in `packages/theme/src/tui/v1-migrate.ts`. Kept in one place because the migration's
//! output is only reproducible if every float operation matches upstream exactly — the
//! 24-step chroma search in `fit_oklch` in particular decides on the 8th bit of a channel.
//!
//! Only the subset upstream actually uses is here: no `RGBA` clone/equals, no gamma knobs.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgb {
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn from_ints(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `Color::Reset` rather than `Color::Rgb(0,0,0)` keeps `transparent` meaning "inherit the
    /// cell below" instead of "paint black" — the v2 `opencode` theme relies on it for
    /// `background.action.primary.base`.
    pub fn to_ratatui(self) -> Color {
        if self.a == 0 {
            Color::Reset
        } else {
            Color::Rgb(self.r, self.g, self.b)
        }
    }

    /// Ratatui has no alpha, so a partially transparent colour is blended against `base`.
    pub fn over(self, base: Rgb) -> Color {
        self.blend(base).to_ratatui()
    }

    pub fn blend(self, base: Rgb) -> Rgb {
        if self.a == 255 {
            return self;
        }
        let mix = |fg: u8, bg: u8| ((u16::from(fg) * u16::from(self.a) + u16::from(bg) * (255 - u16::from(self.a)) + 127) / 255) as u8;
        Rgb::opaque(mix(self.r, base.r), mix(self.g, base.g), mix(self.b, base.b))
    }

    pub fn hex(self) -> String {
        let byte = |value: u8| format!("{value:02x}");
        if self.a == 255 {
            format!("#{}{}{}", byte(self.r), byte(self.g), byte(self.b))
        } else {
            format!("#{}{}{}{}", byte(self.r), byte(self.g), byte(self.b), byte(self.a))
        }
    }

    pub fn from_hex(value: &str) -> Option<Self> {
        let body = value.strip_prefix('#')?;
        if !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |s: &str| u8::from_str_radix(s, 16).ok();
        match body.len() {
            3 | 4 => {
                let doubled: String = body.chars().flat_map(|c| [c, c]).collect();
                Self::from_hex(&format!("#{doubled}"))
            }
            6 => Some(Rgb::opaque(
                channel(&body[0..2])?,
                channel(&body[2..4])?,
                channel(&body[4..6])?,
            )),
            8 => Some(Rgb::from_ints(
                channel(&body[0..2])?,
                channel(&body[2..4])?,
                channel(&body[4..6])?,
                channel(&body[6..8])?,
            )),
            _ => None,
        }
    }

    /// Upstream's `a !== 0` / `a > 0` tests: *any* alpha counts, including 1/255. Named
    /// `has_alpha` rather than something like `is_opaque` on purpose — both call sites in the v1
    /// migration (`detectMode`'s shared-background check and `selectedForeground`) key off
    /// "not fully transparent", and a name suggesting `a == 255` invites the wrong edit.
    pub fn has_alpha(self) -> bool {
        self.a != 0
    }

    /// Rec.601 luma, the same weighting upstream uses for `detectMode` and `selectedForeground`.
    pub fn luminance(self) -> f64 {
        0.299 * f64::from(self.r) / 255.0 + 0.587 * f64::from(self.g) / 255.0 + 0.114 * f64::from(self.b) / 255.0
    }
}

/// xterm-256 palette entry → RGB. Ported from `ansiToRgba` (v1 resolve path) and `ansi`
/// (v1 migration path); the two upstream copies are identical, so there is one here.
pub(crate) fn ansi_to_rgb(code: u8) -> Rgb {
    const BASE16: [Rgb; 16] = [
        Rgb::opaque(0x00, 0x00, 0x00),
        Rgb::opaque(0x80, 0x00, 0x00),
        Rgb::opaque(0x00, 0x80, 0x00),
        Rgb::opaque(0x80, 0x80, 0x00),
        Rgb::opaque(0x00, 0x00, 0x80),
        Rgb::opaque(0x80, 0x00, 0x80),
        Rgb::opaque(0x00, 0x80, 0x80),
        Rgb::opaque(0xc0, 0xc0, 0xc0),
        Rgb::opaque(0x80, 0x80, 0x80),
        Rgb::opaque(0xff, 0x00, 0x00),
        Rgb::opaque(0x00, 0xff, 0x00),
        Rgb::opaque(0xff, 0xff, 0x00),
        Rgb::opaque(0x00, 0x00, 0xff),
        Rgb::opaque(0xff, 0x00, 0xff),
        Rgb::opaque(0x00, 0xff, 0xff),
        Rgb::opaque(0xff, 0xff, 0xff),
    ];
    if code < 16 {
        return BASE16[usize::from(code)];
    }
    if code < 232 {
        let index = u32::from(code) - 16;
        let level = |part: u32| if part == 0 { 0 } else { (part * 40 + 55) as u8 };
        return Rgb::opaque(
            level(index / 36),
            level(index / 6 % 6),
            level(index % 6),
        );
    }
    let gray = (u32::from(code) - 232) * 10 + 8;
    let gray = gray.min(255) as u8;
    Rgb::opaque(gray, gray, gray)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        return value * 12.92;
    }
    1.055 * value.powf(1.0 / 2.4) - 0.055
}

fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.040_45 {
        return value / 12.92;
    }
    ((value + 0.055) / 1.055).powf(2.4)
}

pub(crate) fn rgb_to_oklch(rgb: Rgb) -> Oklch {
    let (red, green, blue) = (
        srgb_to_linear(f64::from(rgb.r) / 255.0),
        srgb_to_linear(f64::from(rgb.g) / 255.0),
        srgb_to_linear(f64::from(rgb.b) / 255.0),
    );
    let l_root = (0.412_221_470_8 * red + 0.536_332_536_3 * green + 0.051_445_992_9 * blue).cbrt();
    let m_root = (0.211_903_498_2 * red + 0.680_699_545_1 * green + 0.107_396_956_6 * blue).cbrt();
    let s_root = (0.088_302_461_9 * red + 0.281_718_837_6 * green + 0.629_978_700_5 * blue).cbrt();
    let lightness = 0.210_454_255_3 * l_root + 0.793_617_785 * m_root - 0.004_072_046_8 * s_root;
    let a = 1.977_998_495_1 * l_root - 2.428_592_205 * m_root + 0.450_593_709_9 * s_root;
    let b = 0.025_904_037_1 * l_root + 0.782_771_766_2 * m_root - 0.808_675_766 * s_root;
    let angle = b.atan2(a) * (180.0 / std::f64::consts::PI);
    Oklch {
        l: lightness,
        c: (a * a + b * b).sqrt(),
        h: if angle < 0.0 { angle + 360.0 } else { angle },
    }
}

fn oklch_to_rgb(color: Oklch) -> (f64, f64, f64) {
    let a = color.c * (color.h * std::f64::consts::PI / 180.0).cos();
    let b = color.c * (color.h * std::f64::consts::PI / 180.0).sin();
    let l_root = color.l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_root = color.l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_root = color.l - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let (l, m, s) = (l_root.powi(3), m_root.powi(3), s_root.powi(3));
    (
        linear_to_srgb(4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s),
        linear_to_srgb(-1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s),
        linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s),
    )
}

fn in_gamut(rgb: (f64, f64, f64)) -> bool {
    let inside = |value: f64| (0.0..=1.0).contains(&value);
    inside(rgb.0) && inside(rgb.1) && inside(rgb.2)
}

/// Upstream keeps the hue and walks chroma down in 0.9 steps until the colour fits sRGB.
/// The result is a different hex than the naive conversion whenever the input is out of gamut,
/// so this loop is load-bearing for migration parity.
fn fit_oklch(color: Oklch) -> Oklch {
    let base = Oklch {
        l: color.l.clamp(0.0, 1.0),
        c: color.c.max(0.0),
        h: ((color.h % 360.0) + 360.0) % 360.0,
    };
    if in_gamut(oklch_to_rgb(base)) {
        return base;
    }
    for index in 0..24 {
        let next = Oklch {
            c: base.c * 0.9_f64.powi(index + 1),
            ..base
        };
        if in_gamut(oklch_to_rgb(next)) {
            return next;
        }
    }
    Oklch { c: 0.0, ..base }
}

pub(crate) fn oklch_to_hex(color: Oklch) -> String {
    let (r, g, b) = oklch_to_rgb(fit_oklch(color));
    let to_hex = |value: f64| format!("{:02x}", (value.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{}{}{}", to_hex(r), to_hex(g), to_hex(b))
}

/// OKLCh interpolation with the shortest hue path, matching upstream `interpolate`.
pub(crate) fn interpolate_oklch(first: Rgb, second: Rgb, amount: f64) -> String {
    let start = rgb_to_oklch(first);
    let end = rgb_to_oklch(second);
    let start_hue = if start.h.is_finite() {
        start.h
    } else if end.h.is_finite() {
        end.h
    } else {
        0.0
    };
    let end_hue = if end.h.is_finite() { end.h } else { start_hue };
    let hue = (((end_hue - start_hue) % 360.0) + 360.0) % 360.0 - 180.0;
    let generated = oklch_to_hex(Oklch {
        l: start.l + (end.l - start.l) * amount,
        c: start.c + (end.c - start.c) * amount,
        h: start_hue + hue * amount,
    });
    let alpha = (f64::from(first.a)
        + (f64::from(second.a) - f64::from(first.a)) * amount)
        .round()
        .clamp(0.0, 255.0) as u8;
    if alpha == 255 {
        generated
    } else {
        format!("{generated}{alpha:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 84/255 is a byte value that is *not* already an sRGB round trip, so it catches a
    // missing or doubled srgb<->linear step instead of passing on a no-op pair like 0/255.
    #[test]
    fn oklch_round_trip_stays_within_one_byte() {
        for (r, g, b) in [(0u8, 0u8, 0u8), (255, 255, 255), (0x48, 0x48, 0x48), (0xfa, 0xb2, 0x83)] {
            let source = Rgb::opaque(r, g, b);
            let back = oklch_to_hex(rgb_to_oklch(source));
            let parsed = Rgb::from_hex(&back).unwrap();
            assert!(
                parsed.r.abs_diff(r) <= 1 && parsed.g.abs_diff(g) <= 1 && parsed.b.abs_diff(b) <= 1,
                "{source:?} round-tripped to {back}"
            );
        }
    }

    #[test]
    fn ansi_cube_and_gray_ramp_match_xterm() {
        assert_eq!(ansi_to_rgb(0), Rgb::opaque(0, 0, 0));
        assert_eq!(ansi_to_rgb(15), Rgb::opaque(255, 255, 255));
        // 16 is cube 0,0,0; 231 is cube 5,5,5; 232 starts the 24-step gray ramp.
        assert_eq!(ansi_to_rgb(16), Rgb::opaque(0, 0, 0));
        assert_eq!(ansi_to_rgb(231), Rgb::opaque(255, 255, 255));
        assert_eq!(ansi_to_rgb(232), Rgb::opaque(8, 8, 8));
        assert_eq!(ansi_to_rgb(255), Rgb::opaque(238, 238, 238));
    }

    #[test]
    fn out_of_gamut_chroma_is_reduced_not_clamped() {
        // Chroma 0.9 at mid lightness is far outside sRGB; upstream reduces chroma instead of
        // clipping channels, so the result must not be a saturated pure hue.
        let hex = oklch_to_hex(Oklch { l: 0.5, c: 0.9, h: 30.0 });
        let rgb = Rgb::from_hex(&hex).unwrap();
        assert!(rgb.r > rgb.b, "{hex} should stay warm");
        assert!(rgb.r != 255 || rgb.b != 0, "{hex} looks channel-clamped");
    }

    #[test]
    fn transparent_alpha_survives_interpolation() {
        let hex = interpolate_oklch(Rgb::from_ints(0, 0, 0, 0), Rgb::opaque(255, 255, 255), 0.5);
        assert_eq!(hex.len(), 9, "{hex} should carry an alpha byte");
    }
}
