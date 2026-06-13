//! Text label passes: rotated sector/subsector watermarks, micro border labels,
//! and the screen-fixed galactic-direction compass.

use std::collections::HashMap;

use tmap_core::astrometrics::{parse_hex, PARSEC_SCALE_X};
use tmap_core::dto::SectorData;

use crate::canvas::{Canvas, TextAlign};

use super::common::{
    hex_parsec, on_screen, sector_center, world_hex, ViewState, C_AMBER, DEFAULT_FONT, SECTOR_H,
    SECTOR_W, SUBSECTOR_H, SUBSECTOR_W,
};

/// Big diagonal watermark labels — the reference rotates sector/subsector
/// names −50° and squishes them to 0.75 width (`sectorName.textStyle`).
const LABEL_ROT: f64 = -50.0 * std::f64::consts::PI / 180.0;
const LABEL_SCALE_X: f64 = 0.75;

/// Sector/subsector name color — the discrete fade ported verbatim from the
/// reference `Stylesheet` (`sectorName.textColor`, default Poster palette):
/// foreground **White** below scale 16, **DarkGray** 16–48, **DimGray** at/above
/// 48. In practice sector names only show at scale ≤ 16 (so they read solid
/// White), and subsector names at 24–64 (so they read DarkGray → DimGray).
fn fade_name_color(scale: f64) -> &'static str {
    if scale < 16.0 {
        "#ffffff" // Color.White (foregroundColor)
    } else if scale < 48.0 {
        "#a9a9a9" // Color.DarkGray
    } else {
        "#696969" // Color.DimGray
    }
}

/// Sector names: rotated watermark at sector centers (font 5.5 parsec).
pub(crate) fn draw_sector_names(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sector_index: &HashMap<(i32, i32), String>,
) {
    let font_px = (5.5 * view.scale).clamp(10.0, 520.0);
    let font = format!("{}px {DEFAULT_FONT}", font_px as i32); // FontInfo(DEFAULT_FONT, 5.5) — Regular
    let color = fade_name_color(view.scale);
    for (&(sx, sy), name) in sector_index {
        let (cx, cy) = view.to_screen(w, h, sector_center(sx, sy));
        if !on_screen(cx, cy, w, h, font_px) {
            continue;
        }
        c.fill_text_rotated(name, cx, cy, color, &font, LABEL_ROT, LABEL_SCALE_X);
    }
}

/// Subsector names: rotated watermark at subsector centers (font 1.5 parsec).
pub(crate) fn draw_subsector_names(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
    let Some(loc) = sector.info.location else {
        return;
    };
    let font_px = (1.5 * view.scale).clamp(10.0, 260.0);
    let font = format!("{}px {DEFAULT_FONT}", font_px as i32); // FontInfo(DEFAULT_FONT, 1.5) — Regular
    let color = fade_name_color(view.scale);
    for ss in &sector.info.subsectors {
        let Some(letter) = ss.index.bytes().next() else {
            continue;
        };
        if !(b'A'..=b'P').contains(&letter) {
            continue;
        }
        let i = (letter - b'A') as i32;
        let (scol, srow) = (i % 4, i / 4);
        let wc = loc.x as f64 * SECTOR_W as f64 + scol as f64 * SUBSECTOR_W as f64 + 4.5;
        let wr = loc.y as f64 * SECTOR_H as f64 + srow as f64 * SUBSECTOR_H as f64 + 5.5;
        let (cx, cy) = view.to_screen(w, h, (wc * PARSEC_SCALE_X as f64, wr));
        if !on_screen(cx, cy, w, h, font_px) {
            continue;
        }
        c.fill_text_rotated(&ss.name, cx, cy, color, &font, LABEL_ROT, LABEL_SCALE_X);
    }
}

/// Border/region labels ("Third Imperium", "Florian League", …) — amber, at the
/// label-position hex (`microBorders.textColor`/`textStyle`). The text comes from
/// the border's explicit `Label` or, lacking one, its allegiance name (resolved
/// backend-side). Bold; size = `microBorders` font (0.25 parsec, floored so it
/// stays legible at the scale-16 minimum, matching the reference's special case).
/// Only wrapped when the border is flagged `WrapLabel`.
pub(crate) fn draw_border_labels(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
    let Some(loc) = sector.info.location else {
        return;
    };
    let size = micro_label_px(0.25, view.scale);
    let font = format!("700 {}px {DEFAULT_FONT}", size as i32);
    for border in &sector.borders {
        let (Some(label), Some(pos)) = (&border.label, &border.label_position) else {
            continue;
        };
        let Some((col, row)) = parse_hex(pos) else {
            continue;
        };
        let (wc, wr) = world_hex(loc.x, loc.y, col, row);
        let (px, py) = hex_parsec(wc, wr);
        // Label nudge: ±0.7 parsec per offset unit (reference LabelOffsetX/Y).
        let (px, py) = (px + border.label_offset.0 as f64 * 0.7, py - border.label_offset.1 as f64 * 0.7);
        let (x, y) = view.to_screen(w, h, (px, py));
        if !on_screen(x, y, w, h, size * 4.0) {
            continue;
        }
        let lines = if border.wrap_label { wrap_label_text(label) } else { vec![label.clone()] };
        draw_label_lines(c, &lines, x, y, size, C_AMBER, &font);
    }
}

/// Standalone hand-placed labels (`<Label>`: "Outrim Void", "Strend Cluster", …) —
/// drawn in the label's own color (default amber) at its `Size` font (small
/// 0.15 / medium 0.25 / large 0.75 parsec, bold). Wrapped only when flagged.
pub(crate) fn draw_sector_labels(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
    let Some(loc) = sector.info.location else {
        return;
    };
    for label in &sector.labels {
        let Some((col, row)) = parse_hex(&label.hex) else {
            continue;
        };
        let parsec = match label.size.as_deref() {
            Some("small") => 0.15,
            Some("large") => 0.75,
            _ => 0.25,
        };
        let size = micro_label_px(parsec, view.scale);
        let font = format!("700 {}px {DEFAULT_FONT}", size as i32);
        let (wc, wr) = world_hex(loc.x, loc.y, col, row);
        let (px, py) = hex_parsec(wc, wr);
        let (px, py) = (px + label.offset.0 as f64 * 0.7, py - label.offset.1 as f64 * 0.7);
        let (x, y) = view.to_screen(w, h, (px, py));
        if !on_screen(x, y, w, h, size * 4.0) {
            continue;
        }
        let color = label.color.as_deref().unwrap_or(C_AMBER);
        let lines = if label.wrap { wrap_label_text(&label.text) } else { vec![label.text.clone()] };
        draw_label_lines(c, &lines, x, y, size, color, &font);
    }
}

/// Micro-label font px from a parsec size: `parsec × scale`, floored so the text
/// stays legible at the scale-16 minimum (the reference bumps the font to 0.6
/// parsec at exactly that scale; we approximate with a flat floor).
fn micro_label_px(parsec: f64, scale: f64) -> f64 {
    (parsec * scale).max(9.0)
}

/// Center a (possibly multi-line) label vertically on its anchor and draw it.
fn draw_label_lines(c: &impl Canvas, lines: &[String], x: f64, y: f64, size: f64, color: &str, font: &str) {
    let top = y - (lines.len() as f64 - 1.0) * size * 0.55;
    for (i, line) in lines.iter().enumerate() {
        c.fill_text(line, x, top + i as f64 * size * 1.1, color, font, TextAlign::Center);
    }
}

/// Wrap a label at runs of whitespace **not** followed by a lowercase letter —
/// the reference `WRAP_REGEX` (`\s+(?![a-z])`). So "Confederation of Duncinae"
/// breaks before "Duncinae" but not before "of".
fn wrap_label_text(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, word) in text.split_whitespace().enumerate() {
        let starts_lower = word.chars().next().is_some_and(|ch| ch.is_ascii_lowercase());
        if i == 0 {
            cur.push_str(word);
        } else if starts_lower {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Screen-fixed COREWARD / RIMWARD / SPINWARD / TRAILING compass labels at the
/// viewport edges (the reference's galactic-direction overlay). Red, like the
/// reference; spinward/trailing read vertically.
pub(crate) fn draw_galactic_directions(c: &impl Canvas, w: f64, h: f64) {
    const COLOR: &str = "rgba(227,39,54,0.78)";
    let font = format!("700 15px {DEFAULT_FONT}");
    let cx = w / 2.0;
    let cy = h / 2.0;
    use std::f64::consts::FRAC_PI_2;
    c.fill_text("COREWARD", cx, 20.0, COLOR, &font, TextAlign::Center);
    c.fill_text("RIMWARD", cx, h - 34.0, COLOR, &font, TextAlign::Center);
    c.fill_text_rotated("SPINWARD", 18.0, cy, COLOR, &font, -FRAC_PI_2, 1.0);
    c.fill_text_rotated("TRAILING", w - 18.0, cy, COLOR, &font, FRAC_PI_2, 1.0);
}
