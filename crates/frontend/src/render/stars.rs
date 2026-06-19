//! Procedural starfield background (the reference's pseudo-random stars).

use crate::canvas::Canvas;

use super::common::{
    hex_parsec, on_screen, scale_interpolate, visible_hex_range, ViewState, STAR_MIN_SCALE,
};

/// **Candy** nebula background — tiles `res/Candy/Nebula.png` to fill the viewport
/// at detail zoom (`DrawNebulaBackground`, `RenderContext.cs:929-963`). The tile is
/// a fixed 2048 px (1024 × `backgroundImageScale` 2.0) anchored to the world origin
/// so it scrolls with panning. Shown when `showNebulaBackground` — i.e. the
/// reference `deepBackgroundOpacity = ScaleInterpolate(1,0,scale,1/8,2) < 0.5`
/// (true once zoomed past `scale ≈ 0.5`, the opposite end from the galaxy).
pub(crate) fn draw_nebula(c: &impl Canvas, view: &ViewState, w: f64, h: f64) {
    if scale_interpolate(1.0, 0.0, view.scale, 0.125, 2.0) >= 0.5 {
        return;
    }
    const TILE: f64 = 2048.0; // 1024 * backgroundImageScale (2.0), fixed screen px
    let (ax, ay) = view.to_screen(w, h, (0.0, 0.0)); // world origin → screen anchor
    let mut ox = ax % TILE;
    let mut oy = ay % TILE;
    if ox > 0.0 {
        ox -= TILE;
    }
    if oy > 0.0 {
        oy -= TILE;
    }
    let nx = (w / TILE).floor() as i32 + 2;
    let ny = (h / TILE).floor() as i32 + 2;
    for ix in 0..nx {
        for iy in 0..ny {
            c.draw_image(
                "/api/res/Candy/Nebula.png",
                ox + ix as f64 * TILE,
                oy + iy as f64 * TILE,
                TILE + 1.0,
                TILE + 1.0,
                1.0,
            );
        }
    }
}

/// Galaxy background image, composited behind the procedural starfield at macro
/// (zoomed-out) view and faded out as you zoom in.
///
/// The reference maps `res/Candy/Galaxy.png` onto a parsec rect spanning roughly
/// `x:[-18257, 18294], y:[-26234, 6228]` (origin (-18257, -26234), size
/// 36551 × 32462). We convert that rect's corners through `view.to_screen` and
/// draw the image into the resulting pixel box — so the galaxy pans/zooms with
/// the map. It is enormous, so only its central region shows at typical
/// zoom-out (expected/correct).
///
/// Opacity follows the reference `ScaleInterpolate(1, 0, scale, 1/8, 2)`: full
/// at `scale <= 1/8`, fading to 0 by `scale == 2`, and capped at ~0.85 so the
/// starfield still reads on top. Above `scale = 2` (detail zoom) it is skipped.
#[allow(dead_code)]
pub(crate) fn draw_galaxy(c: &impl Canvas, view: &ViewState, w: f64, h: f64) {
    if view.scale > 2.0 {
        return; // invisible at detail zoom
    }
    // Reference `deepBackgroundOpacity = ScaleInterpolate(1,0, scale, 1/8, 2)`
    // (logarithmic); capped slightly so the starfield still reads on top.
    let alpha = scale_interpolate(1.0, 0.0, view.scale, 0.125, 2.0) * 0.9;
    if alpha <= 0.0 {
        return;
    }
    // Galaxy.png mapped to this absolute parsec rect (top-left .. bottom-right).
    let (dx, dy) = view.to_screen(w, h, (-18257.0, -26234.0));
    let (bx, by) = view.to_screen(w, h, (18294.0, 6228.0));
    c.draw_image("/api/res/Candy/Galaxy.png", dx, dy, bx - dx, by - dy, alpha);
}

/// Cheap deterministic 2D hash for star placement (stable under pan).
fn hash2(a: i32, b: i32) -> u32 {
    let mut h = (a as u32).wrapping_mul(0x27d4_eb2d) ^ (b as u32).wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// Procedural star field in world space (pans with the map). Skipped when so
/// zoomed out that the cell count explodes.
pub(crate) fn draw_stars(c: &impl Canvas, view: &ViewState, w: f64, h: f64, theme: &super::Theme) {
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
            if !hsh.is_multiple_of(7) {
                continue; // ~14% of cells host a star
            }
            let ox = ((hsh >> 3) & 0xff) as f64 / 255.0 - 0.5;
            let oy = ((hsh >> 11) & 0xff) as f64 / 255.0 - 0.5;
            let (px, py) = hex_parsec(wc, wr);
            let (sx, sy) = view.to_screen(w, h, (px + ox, py + oy));
            if !on_screen(sx, sy, w, h, 2.0) {
                continue;
            }
            let color = theme.stars[((hsh >> 19) & 3) as usize];
            let r = if (hsh >> 27) & 1 == 0 { 0.7 } else { 1.1 };
            c.fill_circle(sx, sy, r, color);
        }
    }
}
