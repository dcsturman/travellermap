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
    // Translucent polity blobs first, so the red border strokes sit on top.
    if opts.borders {
        for v in &ov.borders {
            fill_region(c, view, w, h, v);
        }
    }
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
    // Region names ("THE IMPERIUM", …) and rotated rift names on top.
    if opts.region_names {
        for v in &ov.borders {
            draw_region_label(c, view, w, h, v);
        }
        for v in &ov.rifts {
            draw_rift_label(c, view, w, h, v);
        }
    }
    // Mega-names ("Charted Space", "Core Sophonts") are deferred: that data is
    // not in the `Overlays` stream yet (only borders/routes/rifts/labels).
}

/// Fill color for a polity blob, keyed off the vector's `Name` (case-insensitive
/// substring). Major empires get their canonical hue; unnamed/client regions get
/// a neutral gray. `is_major` distinguishes the two for label styling.
fn polity_fill_color(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("imperium") {
        "#E32736"
    } else if n.contains("aslan") {
        "#ff8c00"
    } else if n.contains("zhodani") {
        "#3a6ea5"
    } else if n.contains("vargr") {
        "#d2a24c"
    } else if n.contains("solomani") {
        "#5aa02c"
    } else if n.contains("hive") || n.contains("hiver") {
        "#9b59b6"
    } else if n.contains("kkree") || n.contains("k'kree") || n.contains("two thousand") {
        "#2e8b57"
    } else {
        "#8a8f99" // client states / unnamed
    }
}

/// Is this a major empire (vs. a minor/client region)? Drives label styling.
fn is_major_polity(name: &str) -> bool {
    polity_fill_color(name) != "#8a8f99"
}

/// World-space vector point → screen, matching `draw_vector`'s transform.
fn vec_point(view: &ViewState, w: f64, h: f64, v: &VectorObject, px: f32, py: f32) -> (f64, f64) {
    let wx = (px - v.origin.0) * v.scale.0;
    let wy = (py - v.origin.1) * v.scale.1;
    view.to_screen(w, h, (wx as f64 * PARSEC_SCALE_X as f64, wy as f64))
}

/// Fill a polity's closed sub-paths as one translucent blob (drawn behind the
/// stroked border). Open sub-paths (none expected for closed polity polygons)
/// are skipped.
fn fill_region(c: &impl Canvas, view: &ViewState, w: f64, h: f64, v: &VectorObject) {
    let polys: Vec<Vec<(f64, f64)>> = v
        .paths
        .iter()
        .filter(|p| p.closed && p.points.len() >= 3)
        .map(|p| p.points.iter().map(|&(px, py)| vec_point(view, w, h, v, px, py)).collect())
        .collect();
    if polys.is_empty() {
        return;
    }
    c.fill_polygons(&polys, polity_fill_color(&v.name), 0.15);
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
            .map(|&(px, py)| vec_point(view, w, h, v, px, py))
            .collect();
        c.stroke_polyline(&pts, color, width, path.closed && !force_open, dash);
    }
}

/// Region name: major empires in bold ALL-CAPS white, minor/client regions in
/// italic non-caps red. Multi-line on `\n`, centered at the vector's label hex.
fn draw_region_label(c: &impl Canvas, view: &ViewState, w: f64, h: f64, v: &VectorObject) {
    if v.name.is_empty() {
        return; // unnamed client-state regions get no major label
    }
    let Some((lx, ly)) = v.label else { return };
    let (sx, sy) = view.to_screen(w, h, (lx as f64 * PARSEC_SCALE_X as f64, ly as f64));
    if !on_screen(sx, sy, w, h, 220.0) {
        return;
    }
    let major = is_major_polity(&v.name);
    let (size, font, color) = if major {
        let size = (1.1 * view.scale).clamp(13.0, 30.0);
        (size, format!("700 {}px {DEFAULT_FONT}", size as i32), "rgba(245,247,255,0.92)")
    } else {
        let size = (0.85 * view.scale).clamp(11.0, 20.0);
        (size, format!("italic 600 {}px {DEFAULT_FONT}", size as i32), "rgba(227,39,54,0.85)")
    };
    let raw = if major { v.name.to_uppercase() } else { v.name.clone() };
    let lines: Vec<&str> = raw.split('\n').map(str::trim).collect();
    let top = sy - (lines.len() as f64 - 1.0) * size * 0.5;
    for (i, line) in lines.iter().enumerate() {
        c.fill_text(line, sx, top + i as f64 * size, color, &font, TextAlign::Center);
    }
}

/// Rift name (Great Rift, …): dim gray, rotated ~35° like the reference.
fn draw_rift_label(c: &impl Canvas, view: &ViewState, w: f64, h: f64, v: &VectorObject) {
    if v.name.is_empty() {
        return;
    }
    let Some((lx, ly)) = v.label else { return };
    let (sx, sy) = view.to_screen(w, h, (lx as f64 * PARSEC_SCALE_X as f64, ly as f64));
    if !on_screen(sx, sy, w, h, 220.0) {
        return;
    }
    let size = (0.9 * view.scale).clamp(12.0, 24.0);
    let font = format!("600 {}px {DEFAULT_FONT}", size as i32);
    let rot = 35.0_f64.to_radians();
    c.fill_text_rotated(&v.name.replace('\n', " "), sx, sy, "rgba(150,155,175,0.8)", &font, rot, 1.0);
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
