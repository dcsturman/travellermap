//! Boundary grids: straight sector/subsector lines, and the cached per-parsec
//! hex grid drawn from world-space `Path2d`s under one view transform.

use std::cell::RefCell;
use std::collections::HashMap;

use tmap_core::astrometrics::PARSEC_SCALE_X;
use web_sys::Path2d;

use crate::canvas::{Canvas, Canvas2d};

use super::common::{
    hex_vertex, sector_in_viewport, visible_hex_range, visible_sectors, ViewState, SECTOR_H,
    SECTOR_W,
};

/// Straight sector/subsector boundary lines at every `step` parsecs (boundaries
/// sit half a hex outside the edge cells).
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
    static GRID_GEOM: RefCell<HashMap<(i32, i32), Path2d>> = RefCell::new(HashMap::new());
}

/// Clear the cached hex-grid geometry (milieu switch).
pub(crate) fn clear_grid_geom() {
    GRID_GEOM.with(|c| c.borrow_mut().clear());
}

/// Build (once) the full hex-grid outline `Path2d` for one sector, in world
/// coords (so it composes with the world→device transform like the borders).
fn build_grid_geom(loc: (i32, i32)) -> Path2d {
    let p = Path2d::new().unwrap();
    for col in 1..=SECTOR_W {
        for row in 1..=SECTOR_H {
            let (wc, wr) = (loc.0 * SECTOR_W + col, loc.1 * SECTOR_H + row);
            let v0 = hex_vertex(wc, wr, 0);
            p.move_to(v0.0, v0.1);
            for k in 1..6 {
                let v = hex_vertex(wc, wr, k);
                p.line_to(v.0, v.1);
            }
            p.close_path();
        }
    }
    p
}

/// Per-parsec hex grid, drawn from cached per-sector world-space `Path2d`s under
/// one view transform — a handful of `add_path` + a single `stroke`, instead of
/// a `stroke()` per on-screen hexagon.
pub(crate) fn draw_hex_grid(
    canvas: &Canvas2d,
    view: &ViewState,
    w: f64,
    h: f64,
    dpr: f64,
    sector_index: &HashMap<(i32, i32), String>,
) {
    let s = view.scale;
    if s / 3f64.sqrt() < 2.0 {
        return; // hexes too small to read
    }
    // Draw the grid for every *charted* sector overlapping the viewport (from
    // the index, not the loaded set) so it shows regardless of world-data load
    // state and never tiles the uncharted void.
    let combined = Path2d::new().unwrap();
    let mut any = false;
    GRID_GEOM.with(|cache| {
        let mut cache = cache.borrow_mut();
        for cell in visible_sectors(view, w, h) {
            if !sector_index.contains_key(&cell) || !sector_in_viewport(cell, view, w, h) {
                continue;
            }
            let g = cache.entry(cell).or_insert_with(|| build_grid_geom(cell));
            combined.add_path(g);
            any = true;
        }
    });
    if !any {
        return;
    }
    let ctx = &canvas.ctx;
    // World(parsec) → device: device = dpr · (w/2 + (p − center)·s), uniform.
    let a = dpr * s;
    let (e, f) = (dpr * (w / 2.0 - view.center.0 * s), dpr * (h / 2.0 - view.center.1 * s));
    ctx.save();
    let _ = ctx.set_transform(a, 0.0, 0.0, a, e, f);
    ctx.set_line_width(1.0 / s); // ~1 css px (the transform scales by s)
    ctx.set_stroke_style_str("rgba(130,150,190,0.22)");
    ctx.stroke_with_path(&combined);
    ctx.restore();
}
