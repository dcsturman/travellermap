//! Macro overlays (galaxy/overview zoom): polity borders, trade routes, rifts,
//! region labels, and capital/homeworld dots — all from the static `Overlays`
//! vector stream (`res/Vectors/`, `res/labels/Worlds.xml`).

use tmap_core::astrometrics::PARSEC_SCALE_X;
use tmap_core::dto::{Overlays, VectorObject};

use crate::canvas::{Canvas, TextAlign};

use super::common::{hex_parsec, on_screen, RenderOptions, ViewState, DEFAULT_FONT};
use super::Theme;

pub(crate) fn draw_overlays(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    ov: &Overlays,
    opts: RenderOptions,
    theme: &Theme,
) {
    // The reference strokes macro borders red (no fill) at macro zoom; the
    // filled polity look comes from the micro-border layer at scale >= 4.
    if theme.show_rift {
        for v in &ov.rifts {
            draw_vector(c, view, w, h, v, theme.rift, 1.0, false, &[]);
        }
    }
    if opts.borders {
        for v in &ov.borders {
            draw_vector(c, view, w, h, v, theme.macro_border, 1.5, false, &[]);
        }
    }
    if opts.routes {
        for v in &ov.routes {
            draw_vector(c, view, w, h, v, theme.route, 1.3, true, &[6.0, 4.0]);
        }
    }
    // Region names ("THE IMPERIUM", …) and rotated rift names on top.
    if opts.region_names {
        for v in &ov.borders {
            draw_region_label(c, view, w, h, v, theme);
        }
        for v in &ov.rifts {
            draw_rift_label(c, view, w, h, v, theme);
        }
    }
    // Mega-names ("Charted Space", "Core Sophonts") are deferred: that data is
    // not in the `Overlays` stream yet (only borders/routes/rifts/labels).
}

/// World-space vector point → screen, matching the reference vector transform.
fn vec_point(view: &ViewState, w: f64, h: f64, v: &VectorObject, px: f32, py: f32) -> (f64, f64) {
    let wx = (px - v.origin.0) * v.scale.0;
    let wy = (py - v.origin.1) * v.scale.1;
    view.to_screen(w, h, (wx as f64 * PARSEC_SCALE_X as f64, wy as f64))
}

#[allow(clippy::too_many_arguments)]
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

/// Macro name size (px) for a polity/rift vector, ported from `macroNames`:
/// `Font` 8/1.4 parsec Bold for **major** (`NamesMajor`), `SmallFont` 5/1.4
/// parsec Regular for **minor**. Scaled by zoom, with a small floor so the
/// label stays legible at the bottom of the macro range.
fn macro_name_px(major: bool, scale: f64) -> f64 {
    let parsec = if major { 8.0 / 1.4 } else { 5.0 / 1.4 };
    (parsec * scale).max(if major { 10.0 } else { 8.0 })
}

/// Region name from a polity `VectorObject`, ported from `DrawMacroNames`:
/// **major** polities (`NamesMajor`) → bold ALL-CAPS white (`textColor`);
/// **minor**/client regions (`NamesMinor`) → regular red (`textHighlightColor`),
/// original case. Only vectors carrying a `NamesMask` flag are labeled.
fn draw_region_label(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    v: &VectorObject,
    theme: &Theme,
) {
    let mo = v.map_options.as_deref().unwrap_or("");
    if v.name.is_empty() || !mo.contains("Names") {
        return;
    }
    let Some((lx, ly)) = v.label else { return };
    let (sx, sy) = view.to_screen(w, h, (lx as f64 * PARSEC_SCALE_X as f64, ly as f64));
    if !on_screen(sx, sy, w, h, 220.0) {
        return;
    }
    let major = mo.contains("NamesMajor");
    let size = macro_name_px(major, view.scale);
    let (font, color, raw) = if major {
        (
            format!("700 {}px {DEFAULT_FONT}", size as i32),
            theme.macro_name,
            v.name.to_uppercase(),
        )
    } else {
        (
            format!("{}px {DEFAULT_FONT}", size as i32),
            theme.highlight,
            v.name.clone(),
        )
    };
    let lines: Vec<&str> = raw.split('\n').map(str::trim).collect();
    let top = sy - (lines.len() as f64 - 1.0) * size * 0.5;
    for (i, line) in lines.iter().enumerate() {
        c.fill_text(
            line,
            sx,
            top + i as f64 * size,
            color,
            &font,
            TextAlign::Center,
        );
    }
}

/// Rift name (Great Rift, …), ported from `DrawMacroNames`: same major/minor
/// font + color as regions, but rotated 35°.
fn draw_rift_label(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    v: &VectorObject,
    theme: &Theme,
) {
    let mo = v.map_options.as_deref().unwrap_or("");
    if v.name.is_empty() || !mo.contains("Names") {
        return;
    }
    let Some((lx, ly)) = v.label else { return };
    let (sx, sy) = view.to_screen(w, h, (lx as f64 * PARSEC_SCALE_X as f64, ly as f64));
    if !on_screen(sx, sy, w, h, 220.0) {
        return;
    }
    let major = mo.contains("NamesMajor");
    let size = macro_name_px(major, view.scale);
    let (font, color) = if major {
        (
            format!("700 {}px {DEFAULT_FONT}", size as i32),
            theme.macro_name,
        )
    } else {
        (format!("{}px {DEFAULT_FONT}", size as i32), theme.highlight)
    };
    c.fill_text_rotated(
        &v.name.replace('\n', " "),
        sx,
        sy,
        color,
        &font,
        35.0_f64.to_radians(),
        1.0,
        1.0,
    );
}

/// Galaxy-scale labels (`Overlays.mega_labels`): "Charted Space", "Core
/// Sophonts", … shown only at the most zoomed-out view. White; major labels bold,
/// minor labels smaller italic. Font scales to a roughly constant on-screen size
/// (reference `megaNameScaleFactor = min(35, 0.75/scale)`).
pub(crate) fn draw_mega_labels(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    ov: &Overlays,
    theme: &Theme,
) {
    let unit = 0.75_f64.min(35.0 * view.scale); // = scaleFactor · scale
    let major_px = (24.0 * unit).max(8.0) as i32;
    let minor_px = (18.0 * unit).max(7.0) as i32;
    let major_font = format!("700 {major_px}px {DEFAULT_FONT}");
    let minor_font = format!("italic {minor_px}px {DEFAULT_FONT}");
    for label in &ov.mega_labels {
        // X is in raw (un-compressed) parsec units; apply the x-compression
        // (matches the vector/region label convention).
        let (sx, sy) = view.to_screen(
            w,
            h,
            (label.x as f64 * PARSEC_SCALE_X as f64, label.y as f64),
        );
        if !on_screen(sx, sy, w, h, 320.0) {
            continue;
        }
        let (font, size) = if label.minor {
            (&minor_font, minor_px as f64)
        } else {
            (&major_font, major_px as f64)
        };
        let lines: Vec<&str> = label.text.split('\n').collect();
        let top = sy - (lines.len() as f64 - 1.0) * size * 0.6;
        for (i, line) in lines.iter().enumerate() {
            c.fill_text(
                line,
                sx,
                top + i as f64 * size * 1.15,
                theme.mega_name,
                font,
                TextAlign::Center,
            );
        }
    }
}

/// Minor region labels (`Overlays.minor_labels`, from `minor_labels.tab`), drawn
/// over the macro view (scale 0.5–4). Ported from `DrawMacroNames`' minor block:
/// a label's `minor` flag picks `macroNames.SmallFont` (5/1.4 parsec, regular,
/// `textColor` white) when true, else `MediumFont` (6.5/1.4 parsec, **italic**,
/// `textHighlightColor` **red**) — so the common `Minor=False` region names
/// ("Mixed Client States", "Aslan Colonies", …) read as red italic. `x`/`y` are
/// in the same x-compressed world space as the mega labels (straight to_screen).
pub(crate) fn draw_minor_labels(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    ov: &Overlays,
    theme: &Theme,
) {
    for label in &ov.minor_labels {
        // X is in raw (un-compressed) parsec units, like the vector region
        // labels — apply the x-compression to land them on the map.
        let (sx, sy) = view.to_screen(
            w,
            h,
            (label.x as f64 * PARSEC_SCALE_X as f64, label.y as f64),
        );
        if !on_screen(sx, sy, w, h, 220.0) {
            continue;
        }
        // FontInfo sizes are in parsecs → px by × scale, with a legibility floor.
        let parsec = if label.minor { 5.0 / 1.4 } else { 6.5 / 1.4 };
        let size = (parsec * view.scale).max(8.0) as i32;
        let (font, color) = if label.minor {
            (format!("{size}px {DEFAULT_FONT}"), theme.macro_name)
        } else {
            (format!("italic {size}px {DEFAULT_FONT}"), theme.highlight)
        };
        let size = size as f64;
        let lines: Vec<&str> = label.text.split('\n').map(str::trim).collect();
        let top = sy - (lines.len() as f64 - 1.0) * size * 0.5;
        for (i, line) in lines.iter().enumerate() {
            c.fill_text(
                line,
                sx,
                top + i as f64 * size,
                color,
                &font,
                TextAlign::Center,
            );
        }
    }
}

/// Capitals + homeworlds (`Overlays.labels`): a Wheat dot at the world hex with
/// a red name label offset by its `bias` (reference `WorldObject.Paint`).
pub(crate) fn draw_world_labels(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    ov: &Overlays,
    theme: &Theme,
) {
    let font = format!("600 13px {DEFAULT_FONT}");
    let r = (1.5 * view.scale).clamp(2.0, 6.0);
    for label in &ov.labels {
        let (x, y) = view.to_screen(w, h, hex_parsec(label.coord.x, label.coord.y));
        if !on_screen(x, y, w, h, 140.0) {
            continue;
        }
        c.fill_circle(x, y, r, theme.capital_fill); // Color.Wheat
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
        let top = ly
            - (n - 1.0)
                * line_h
                * if by < 0.0 {
                    1.0
                } else if by > 0.0 {
                    0.0
                } else {
                    0.5
                };
        for (i, line) in lines.iter().enumerate() {
            c.fill_text(
                line,
                lx,
                top + i as f64 * line_h,
                theme.capital,
                &font,
                align,
            );
        }
    }
}
