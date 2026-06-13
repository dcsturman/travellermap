//! Macro overlays (galaxy/overview zoom): polity borders, trade routes, rifts,
//! region labels, and capital/homeworld dots — all from the static `Overlays`
//! vector stream (`res/Vectors/`, `res/labels/Worlds.xml`).

use tmap_core::astrometrics::PARSEC_SCALE_X;
use tmap_core::dto::{Overlays, VectorObject};

use crate::canvas::{Canvas, TextAlign};

use super::common::{
    hex_parsec, on_screen, RenderOptions, ViewState, C_BORDER, C_RIFT, C_ROUTE, DEFAULT_FONT,
};

pub(crate) fn draw_overlays(c: &impl Canvas, view: &ViewState, w: f64, h: f64, ov: &Overlays, opts: RenderOptions) {
    for v in &ov.rifts {
        draw_vector(c, view, w, h, v, C_RIFT, 1.0, false, &[]);
    }
    if opts.borders {
        for v in &ov.borders {
            draw_vector(c, view, w, h, v, C_BORDER, 1.5, false, &[]);
        }
    }
    if opts.routes {
        for v in &ov.routes {
            draw_vector(c, view, w, h, v, C_ROUTE, 1.3, true, &[6.0, 4.0]);
        }
    }
    // Region names ("THE IMPERIUM", …) on top.
    if opts.region_names {
        for v in &ov.borders {
            draw_region_label(c, view, w, h, v);
        }
    }
}

fn draw_vector(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    v: &VectorObject,
    color: &str,
    width: f64,
    force_open: bool,
    dash: &[f64],
) {
    for path in &v.paths {
        let pts: Vec<(f64, f64)> = path
            .points
            .iter()
            .map(|&(px, py)| {
                let wx = (px - v.origin.0) * v.scale.0;
                let wy = (py - v.origin.1) * v.scale.1;
                view.to_screen(w, h, (wx as f64 * PARSEC_SCALE_X as f64, wy as f64))
            })
            .collect();
        c.stroke_polyline(&pts, color, width, path.closed && !force_open, dash);
    }
}

fn draw_region_label(c: &impl Canvas, view: &ViewState, w: f64, h: f64, v: &VectorObject) {
    if v.name.is_empty() {
        return; // unnamed client-state regions get no major label
    }
    let Some((lx, ly)) = v.label else { return };
    let (sx, sy) = view.to_screen(w, h, (lx as f64 * PARSEC_SCALE_X as f64, ly as f64));
    if !on_screen(sx, sy, w, h, 200.0) {
        return;
    }
    let size = 15.0_f64;
    let font = format!("600 {}px system-ui, sans-serif", size as i32);
    let lines: Vec<&str> = v.name.split('\n').map(str::trim).collect();
    let top = sy - (lines.len() as f64 - 1.0) * size * 0.5;
    for (i, line) in lines.iter().enumerate() {
        c.fill_text(
            line,
            sx,
            top + i as f64 * size,
            "rgba(255,255,255,0.88)",
            &font,
            TextAlign::Center,
        );
    }
}

/// Capitals + homeworlds (`Overlays.labels`): a Wheat dot at the world hex with
/// a red name label offset by its `bias` (reference `WorldObject.Paint`).
pub(crate) fn draw_world_labels(c: &impl Canvas, view: &ViewState, w: f64, h: f64, ov: &Overlays) {
    let font = format!("600 13px {DEFAULT_FONT}");
    let r = (1.5 * view.scale).clamp(2.0, 6.0);
    for label in &ov.labels {
        let (x, y) = view.to_screen(w, h, hex_parsec(label.coord.x, label.coord.y));
        if !on_screen(x, y, w, h, 140.0) {
            continue;
        }
        c.fill_circle(x, y, r, "#f5deb3"); // Color.Wheat
        let (bx, by) = (label.bias.0 as f64, label.bias.1 as f64);
        let off = r + 4.0;
        let (lx, ly) = (x + bx * off, y + by * off);
        let align = if bx > 0.0 {
            TextAlign::Left
        } else if bx < 0.0 {
            TextAlign::Right
        } else {
            TextAlign::Center
        };
        let lines: Vec<&str> = label.name.split('\n').collect();
        let line_h = 14.0;
        let n = lines.len() as f64;
        // Anchor the text block on the dot's bias side (above if by<0, below if
        // by>0, centered if 0).
        let top = ly - (n - 1.0) * line_h * if by < 0.0 { 1.0 } else if by > 0.0 { 0.0 } else { 0.5 };
        for (i, line) in lines.iter().enumerate() {
            c.fill_text(line, lx, top + i as f64 * line_h, "#e8636f", &font, align);
        }
    }
}
