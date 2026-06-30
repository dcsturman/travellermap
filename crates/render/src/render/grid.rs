//! Boundary grids: straight sector/subsector lines, and the cached per-parsec
//! hex grid drawn from world-space [`Geometry`] under one view transform.

use std::cell::RefCell;
use std::collections::HashMap;

use tmap_core::astrometrics::PARSEC_SCALE_X;

use crate::canvas::{Affine, Canvas, Geometry, PathBuilder, StrokeStyle, TextAlign};

use super::common::{
    grid_color, hex_parsec, hex_vertex, on_screen, sector_in_viewport, visible_hex_range,
    visible_sectors, ViewState, CONTENT_SCALE, SECTOR_H, SECTOR_W,
};
use super::Theme;

/// `numberAllHexes` — print the hex coordinate in *every* visible hex (top-center,
/// `hexNumber` color), for the blueprint Draft/FASA/Terminal styles. FASA uses
/// subsector-relative coords (col 1–8 / row 1–10); the others use sector hexes.
/// (Local hex is derived in the frontend's `world_hex` convention, not tmap-core's
/// `coordinates_to_location`, which uses a different absolute origin.)
pub(crate) fn draw_all_hex_numbers(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    theme: &Theme,
) {
    let s = view.scale;
    let (c0, c1, r0, r1) = visible_hex_range(view, w, h);
    let font_px = (0.10 * s * CONTENT_SCALE).max(6.0);
    let font = format!("{}px {}", font_px as i32, theme.font);
    let dy = -0.5 * s; // top edge of the hex
    for wc in c0..=c1 {
        for wr in r0..=r1 {
            let (cx, cy) = view.to_screen(w, h, hex_parsec(wc, wr));
            if !on_screen(cx, cy, w, h, font_px * 3.0) {
                continue;
            }
            // Sector-local hex (1-based col/row) in the world_hex convention.
            let col = (wc - 1).rem_euclid(SECTOR_W) + 1;
            let row = (wr - 1).rem_euclid(SECTOR_H) + 1;
            let label = if theme.subsector_hex_coords {
                format!(
                    "{:02}{:02}",
                    (col - 1).rem_euclid(8) + 1,
                    (row - 1).rem_euclid(10) + 1
                )
            } else {
                format!("{col:02}{row:02}")
            };
            // Reference TopCenter (top baseline at the hex's top edge).
            c.fill_text_top(
                &label,
                cx,
                cy + dy,
                theme.text_hex,
                &font,
                TextAlign::Center,
            );
        }
    }
}

/// Straight sector/subsector boundary lines at every `step` parsecs (boundaries
/// sit half a hex outside the edge cells).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_grid_lines(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    step_col: i32,
    step_row: i32,
    color: &str,
    width: f64,
) {
    let (wc0, wc1, wr0, wr1) = visible_hex_range(view, w, h);
    let kx0 = (wc0 as f64 / step_col as f64).floor() as i32 - 1;
    let kx1 = (wc1 as f64 / step_col as f64).ceil() as i32 + 1;
    for k in kx0..=kx1 {
        let px = (k as f64 * step_col as f64 + 0.5) * PARSEC_SCALE_X as f64;
        let (sx, _) = view.to_screen(w, h, (px, 0.0));
        if sx >= -1.0 && sx <= w + 1.0 {
            c.stroke_polyline(&[(sx, 0.0), (sx, h)], color, width, false, &[]);
        }
    }
    let ky0 = (wr0 as f64 / step_row as f64).floor() as i32 - 1;
    let ky1 = (wr1 as f64 / step_row as f64).ceil() as i32 + 1;
    for k in ky0..=ky1 {
        let py = k as f64 * step_row as f64 + 0.5;
        let (_, sy) = view.to_screen(w, h, (0.0, py));
        if sy >= -1.0 && sy <= h + 1.0 {
            c.stroke_polyline(&[(0.0, sy), (w, sy)], color, width, false, &[]);
        }
    }
}

// Persistent per-sector hex-grid geometry: one `Path2d` of all 1280 hex
// outlines in world (parsec) coords, built once per sector. Same trick as the
// border cache — the old grid issued a separate `stroke()` per on-screen
// hexagon (thousands of wasm→JS crossings per frame, the zoomed-in hot layer);
// now we stroke cached world-space paths under one view transform. (Clear on
// milieu switch, like `SECTOR_GEOM`.)
thread_local! {
    static GRID_GEOM: RefCell<HashMap<(i32, i32), Geometry>> = RefCell::new(HashMap::new());
    /// The per-frame *combined* grid path (all visible sectors merged), cached by
    /// the visible-sector set. Without this, `draw_hex_grid` rebuilt a fresh
    /// `Geometry` every frame — and since a fresh `Geometry` carries no
    /// materialized `Path2d`, the frontend re-replayed every hex edge into a new
    /// `Path2d` each frame (the same per-frame cost the border cache avoids). With
    /// it, the combined `Geometry` persists across frames so its `Path2d` is
    /// memoized too (see `tmap_render::canvas::Geometry::backend_cache`).
    static GRID_COMBINED: RefCell<Option<(u64, Geometry)>> = const { RefCell::new(None) };
}

/// Clear the cached hex-grid geometry (milieu switch).
pub(crate) fn clear_grid_geom() {
    GRID_GEOM.with(|c| c.borrow_mut().clear());
    GRID_COMBINED.with(|c| *c.borrow_mut() = None);
}

/// Build (once) the full hex-grid outline geometry for one sector, in world
/// coords (so it composes with the world→device transform like the borders).
fn build_grid_geom(loc: (i32, i32)) -> Geometry {
    let p = PathBuilder::new();
    for col in 1..=SECTOR_W {
        for row in 1..=SECTOR_H {
            let (wc, wr) = (loc.0 * SECTOR_W + col, loc.1 * SECTOR_H + row);
            let v0 = hex_vertex(wc, wr, 0);
            p.move_to(v0.0, v0.1);
            for k in 1..6 {
                let v = hex_vertex(wc, wr, k);
                p.line_to(v.0, v.1);
            }
            p.close();
        }
    }
    p.finish()
}

/// Per-parsec hex grid, drawn from cached per-sector world-space geometries under
/// one view transform — a handful of `add` + a single `stroke_geometry`, instead
/// of a `stroke()` per on-screen hexagon.
pub(crate) fn draw_hex_grid(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    dpr: f64,
    sector_index: &HashMap<(i32, i32), String>,
    grid_override: Option<&str>,
) {
    let s = view.scale;
    if s / 3f64.sqrt() < 2.0 {
        return; // hexes too small to read
    }
    // Draw the grid for every *charted* sector overlapping the viewport (from
    // the index, not the loaded set) so it shows regardless of world-data load
    // state and never tiles the uncharted void.
    let mut cells: Vec<(i32, i32)> = visible_sectors(view, w, h)
        .into_iter()
        .filter(|cell| sector_index.contains_key(cell) && sector_in_viewport(*cell, view, w, h))
        .collect();
    if cells.is_empty() {
        return;
    }
    cells.sort_unstable();
    // Cache the combined path by the visible-sector set so it's rebuilt only when
    // that set changes (pan into a new sector / zoom), not on every frame — the
    // persistent `Geometry` then keeps its memoized `Path2d` across frames.
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hsh = std::collections::hash_map::DefaultHasher::new();
        cells.hash(&mut hsh);
        hsh.finish()
    };
    // World(parsec) → device: device = dpr · (w/2 + (p − center)·s), uniform.
    let m = Affine::scale_translate(
        dpr * s,
        dpr * (w / 2.0 - view.center.0 * s),
        dpr * (h / 2.0 - view.center.1 * s),
    );
    // Theme override (flat color) or the scale-faded gray `gridColor`. Width is
    // ~1 css px (the transform scales the world-space width by s).
    let color = grid_override.map_or_else(|| grid_color(s), str::to_string);
    GRID_COMBINED.with(|combined_cache| {
        let mut combined_cache = combined_cache.borrow_mut();
        if combined_cache.as_ref().map(|(k, _)| *k) != Some(key) {
            let combined = PathBuilder::new();
            GRID_GEOM.with(|cache| {
                let mut cache = cache.borrow_mut();
                for cell in &cells {
                    combined.add(cache.entry(*cell).or_insert_with(|| build_grid_geom(*cell)));
                }
            });
            *combined_cache = Some((key, combined.finish()));
        }
        let combined = &combined_cache.as_ref().unwrap().1;
        c.stroke_geometry(combined, m, &color, &StrokeStyle::plain(1.0 / s), None);
    });
}
