//! Pure colour conversions used by chart drawings and the drawing-colour UI.
//!
//! Picker interaction state belongs to the colour window; the maths lives here
//! so the chart render path and the window agree on parsed/serialized values.

use gpui::{Hsla, Rgba};

/// Parse `#RRGGBB` (or `RRGGBB`) into an `Hsla` colour.
pub fn hex_to_hsla(hex: &str) -> Option<Hsla> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(gpui::rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | 0xff).into())
}

/// Serialize an `Hsla` colour as uppercase `#RRGGBB`.
pub fn hsla_to_hex(color: Hsla) -> String {
    let rgb = color.to_rgb();
    format!(
        "#{:02X}{:02X}{:02X}",
        (rgb.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Convert HSV (all components in `0..=1`) to `Hsla`.
pub fn hsv_to_hsla(hue: f32, saturation: f32, value: f32) -> Hsla {
    let hue = hue.rem_euclid(1.0);
    let saturation = saturation.clamp(0.0, 1.0);
    let value = value.clamp(0.0, 1.0);
    let chroma = value * saturation;
    let sector = hue * 6.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector.floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    Hsla::from(Rgba {
        r: r + m,
        g: g + m,
        b: b + m,
        a: 1.0,
    })
}

/// Convert an `Hsla` colour to HSV. Achromatic colours report hue `0.0`; the
/// colour window keeps its own hue so dragging value/saturation is stable.
pub fn hsla_to_hsv(color: Hsla) -> (f32, f32, f32) {
    let rgb = color.to_rgb();
    let max = rgb.r.max(rgb.g.max(rgb.b));
    let min = rgb.r.min(rgb.g.min(rgb.b));
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == rgb.r {
        ((rgb.g - rgb.b) / delta).rem_euclid(6.0) / 6.0
    } else if max == rgb.g {
        (((rgb.b - rgb.r) / delta) + 2.0) / 6.0
    } else {
        (((rgb.r - rgb.g) / delta) + 4.0) / 6.0
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for hex in [
            "#C4E855", "#00F0FF", "#FFD600", "#FF0055", "#FFFFFF", "#000000", "#2EBD85",
        ] {
            let parsed = hex_to_hsla(hex).expect("valid hex");
            assert_eq!(hsla_to_hex(parsed), hex);
        }
    }

    #[test]
    fn invalid_hex_is_rejected() {
        assert!(hex_to_hsla("").is_none());
        assert!(hex_to_hsla("#FFF").is_none());
        assert!(hex_to_hsla("#GGGGGG").is_none());
    }

    #[test]
    fn hsv_hue_round_trips_for_saturated_colours() {
        for hue in [0.0, 0.12, 0.33, 0.58, 0.87] {
            let color = hsv_to_hsla(hue, 0.8, 0.9);
            let (parsed, saturation, value) = hsla_to_hsv(color);
            assert!((parsed - hue).abs() < 0.01, "{hue} -> {parsed}");
            assert!((saturation - 0.8).abs() < 0.01);
            assert!((value - 0.9).abs() < 0.01);
        }
    }
}
