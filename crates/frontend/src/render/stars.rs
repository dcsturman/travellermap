//! Procedural starfield background (the reference's pseudo-random stars).

use crate::canvas::Canvas;

use super::common::{hex_parsec, on_screen, visible_hex_range, ViewState, STAR_MIN_SCALE};

/// Cheap deterministic 2D hash for star placement (stable under pan).
fn hash2(a: i32, b: i32) -> u32 {
    let mut h = (a as u32).wrapping_mul(0x27d4_eb2d) ^ (b as u32).wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// Procedural star field in world space (pans with the map). Skipped when so
/// zoomed out that the cell count explodes.
pub(crate) fn draw_stars(c: &impl Canvas, view: &ViewState, w: f64, h: f64) {
    if view.scale < STAR_MIN_SCALE {
        return;
    }
    let (wc0, wc1, wr0, wr1) = visible_hex_range(view, w, h);
    if (wc1 - wc0) as i64 * (wr1 - wr0) as i64 > 45_000 {
        return; // too many cells to iterate cheaply when zoomed out
    }
    for wc in wc0..=wc1 {
        for wr in wr0..=wr1 {
            let hsh = hash2(wc, wr);
            if hsh % 7 != 0 {
                continue; // ~14% of cells host a star
            }
            let ox = ((hsh >> 3) & 0xff) as f64 / 255.0 - 0.5;
            let oy = ((hsh >> 11) & 0xff) as f64 / 255.0 - 0.5;
            let (px, py) = hex_parsec(wc, wr);
            let (sx, sy) = view.to_screen(w, h, (px + ox, py + oy));
            if !on_screen(sx, sy, w, h, 2.0) {
                continue;
            }
            let color = match (hsh >> 19) & 3 {
                0 => "rgba(170,180,205,0.35)",
                1 => "rgba(205,215,235,0.55)",
                2 => "rgba(230,235,250,0.75)",
                _ => "rgba(255,255,255,0.9)",
            };
            let r = if (hsh >> 27) & 1 == 0 { 0.7 } else { 1.1 };
            c.fill_circle(sx, sy, r, color);
        }
    }
}
