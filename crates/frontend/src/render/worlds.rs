//! Detail-tier world rendering: the cached disc/zone/outline "dot" layer, and
//! the state-batched glyph passes (hex#, starport, gas giant, bases, UWP,
//! allegiance, name) — a faithful port of the reference `DrawWorld` layout.

use std::cell::RefCell;
use std::collections::HashMap;

use tmap_core::astrometrics::parse_hex;
use tmap_core::dto::{SectorData, World};
use web_sys::Path2d;

use crate::canvas::Canvas2d;
use crate::glyph;

use super::common::{
    hex_parsec, on_screen, sector_in_viewport, world_hex, ViewState, ALLEGIANCE_MIN_SCALE, C_AMBER,
    C_RED, C_WATER, C_DRY, CONTENT_SCALE, DEFAULT_FONT, WORLD_BASIC_SCALE, WORLD_FULL_SCALE,
    WORLD_UWP_SCALE,
};

/// World disc radius (parsec), reference `discRadius`: **0.2** at the dot-only
/// tier (Dotmap, scale < `WORLD_BASIC_SCALE`), **0.1** once world detail is
/// drawn. The travel-zone broken ring sits 0.1 parsec outside the disc.
fn disc_radius(dotmap: bool) -> f64 {
    if dotmap {
        0.2
    } else {
        0.1
    }
}

/// Travel-zone arc radius (parsec) — a single open-bottom arc encircling the
/// world's hex content (amber/red), not a full ring.
const ZONE_R: f64 = 0.4;

/// Cached per-sector "dot tier" geometry (scale < `WORLD_BASIC_SCALE`, no text):
/// world discs + travel-zone rings grouped by color into `Path2d`s in world
/// coords. Built once per sector (per `more_colors` setting) so a zoomed-out
/// frame with thousands of worlds issues a few `fill`/`stroke`s instead of a
/// `fill_circle`/`stroke_arc` per world. *(Clear on milieu switch.)*
struct SectorDots {
    more_colors: bool,
    dotmap: bool, // disc radius tier the geometry was built for
    discs: Vec<(String, Path2d)>,
    outlines: Vec<(String, Path2d)>,
    zones: Vec<(String, Path2d)>,
}
thread_local! {
    static SECTOR_DOTS: RefCell<HashMap<(i32, i32), SectorDots>> = RefCell::new(HashMap::new());
}

/// Clear the cached world-dot geometry (milieu switch).
pub(crate) fn clear_sector_dots() {
    SECTOR_DOTS.with(|c| c.borrow_mut().clear());
}

/// A world with an unknown UWP renders a special glyph instead of a disc/UWP
/// (reference `World.IsPlaceholder`).
fn is_placeholder(world: &World) -> bool {
    world.uwp == "???????-?" || world.uwp == "XXXXXXX-X"
}

/// A placeholder that is also a deep-space anomaly/station (reference
/// `World.IsAnomaly` = `HasCode("{Anomaly}")`) — draws the red crosshair.
fn is_anomaly(world: &World) -> bool {
    world.codes().any(|c| c == "{Anomaly}")
}

/// Placeholder / anomaly worlds: a `*` (unknown world — white, Georgia) or `⌖`
/// U+2316 POSITION INDICATOR (deep-space anomaly/station — red) centered in the
/// hex, standing in for the disc. Ported from the reference `placeholder` /
/// `anomaly` style elements (`content`/font/color/position). Drawn whenever
/// worlds are visible (the glyph replaces the dot at every detail tier).
pub(crate) fn draw_placeholder_glyphs(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, sectors: &[&SectorData]) {
    let ctx = &canvas.ctx;
    let s = view.scale;
    let cs = s * CONTENT_SCALE;
    // Reference FontInfo sizes are 0.6 parsec (same convention as our other
    // glyph fonts, ~size·cs px).
    let star_font = format!("{}px Georgia, 'Times New Roman', serif", (0.6 * cs).max(6.0) as i32);
    let anomaly_font = format!("{}px 'Arial Unicode MS', 'Segoe UI Symbol', sans-serif", (0.6 * cs).max(6.0) as i32);
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    for sector in sectors {
        let Some(loc) = sector.info.location else { continue };
        for world in &sector.worlds {
            if !is_placeholder(world) {
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else { continue };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (x, y) = view.to_screen(w, h, hex_parsec(wc, wr));
            if !on_screen(x, y, w, h, cs) {
                continue;
            }
            // Placeholders are rare, so set state per glyph (no batching needed).
            if is_anomaly(world) {
                ctx.set_font(&anomaly_font);
                ctx.set_fill_style_str(C_RED);
                let _ = ctx.fill_text("\u{2316}", x, y); // position (0, 0)
            } else {
                ctx.set_font(&star_font);
                ctx.set_fill_style_str("#ffffff");
                let _ = ctx.fill_text("*", x, y + 0.17 * s); // position (0, 0.17)
            }
        }
    }
}

fn build_sector_dots(sector: &SectorData, more_colors: bool, dotmap: bool) -> SectorDots {
    use std::f64::consts::{PI, TAU};
    let disc_r = disc_radius(dotmap);
    let mut discs: HashMap<String, Path2d> = HashMap::new();
    let mut outlines: HashMap<String, Path2d> = HashMap::new();
    let mut zones: HashMap<String, Path2d> = HashMap::new();
    if let Some(loc) = sector.info.location {
        let add_circle = |map: &mut HashMap<String, Path2d>, color: &str, cx: f64, cy: f64| {
            let p = map.entry(color.to_owned()).or_insert_with(|| Path2d::new().unwrap());
            p.move_to(cx + disc_r, cy);
            let _ = p.arc(cx, cy, disc_r, 0.0, TAU);
        };
        for world in &sector.worlds {
            // Placeholder/anomaly worlds get a special glyph instead of a disc
            // (drawn by `draw_placeholder_glyphs`), so skip their dot geometry.
            if is_placeholder(world) {
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else { continue };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (cx, cy) = hex_parsec(wc, wr);
            // Travel zone: a single open-bottom arc behind the disc (amber/red).
            let zc = match world.zone.as_str() { "A" => Some(C_AMBER), "R" => Some(C_RED), _ => None };
            if let Some(zc) = zc {
                let (a0, a1) = (PI - 0.384, 2.0 * PI + 0.384);
                let p = zones.entry(zc.to_owned()).or_insert_with(|| Path2d::new().unwrap());
                p.move_to(cx + ZONE_R * a0.cos(), cy + ZONE_R * a0.sin());
                let _ = p.arc(cx, cy, ZONE_R, a0, a1);
            }
            let (fill, outline) = world_colors(world, more_colors);
            add_circle(&mut discs, fill, cx, cy);
            if let Some(oc) = outline {
                add_circle(&mut outlines, oc, cx, cy);
            }
        }
    }
    SectorDots {
        more_colors,
        dotmap,
        discs: discs.into_iter().collect(),
        outlines: outlines.into_iter().collect(),
        zones: zones.into_iter().collect(),
    }
}

/// Dot-tier worlds (scale < `WORLD_BASIC_SCALE`): batched discs + zone rings
/// from the per-sector cache, drawn under one view transform.
pub(crate) fn draw_world_dots(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, dpr: f64, sectors: &[&SectorData], more_colors: bool) {
    let s = view.scale;
    let dotmap = s < WORLD_BASIC_SCALE; // bigger discs when no per-world detail
    let mut discs: HashMap<String, Path2d> = HashMap::new();
    let mut outlines: HashMap<String, Path2d> = HashMap::new();
    let mut zones: HashMap<String, Path2d> = HashMap::new();
    let merge = |dst: &mut HashMap<String, Path2d>, src: &[(String, Path2d)]| {
        for (c, p) in src {
            dst.entry(c.clone()).or_insert_with(|| Path2d::new().unwrap()).add_path(p);
        }
    };
    SECTOR_DOTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        for sector in sectors {
            let Some(loc) = sector.info.location else { continue };
            if !sector_in_viewport((loc.x, loc.y), view, w, h) {
                continue;
            }
            let dots = cache.entry((loc.x, loc.y)).or_insert_with(|| build_sector_dots(sector, more_colors, dotmap));
            if dots.more_colors != more_colors || dots.dotmap != dotmap {
                *dots = build_sector_dots(sector, more_colors, dotmap); // tier/colors changed
            }
            merge(&mut zones, &dots.zones);
            merge(&mut discs, &dots.discs);
            merge(&mut outlines, &dots.outlines);
        }
    });
    let ctx = &canvas.ctx;
    let a = dpr * s;
    let (e, f) = (dpr * (w / 2.0 - view.center.0 * s), dpr * (h / 2.0 - view.center.1 * s));
    let cs = s * CONTENT_SCALE;
    ctx.save();
    let _ = ctx.set_transform(a, 0.0, 0.0, a, e, f);
    // Zones first (behind), then disc fills, then vacuum outlines. Line widths
    // are css px ÷ s (the transform scales by s).
    ctx.set_line_width(((0.03 * cs).max(1.5)) / s);
    for (color, path) in &zones {
        ctx.set_stroke_style_str(color);
        ctx.stroke_with_path(path);
    }
    for (color, path) in &discs {
        ctx.set_fill_style_str(color);
        ctx.fill_with_path_2d(path);
    }
    ctx.set_line_width(((0.02 * cs).max(1.0)) / s);
    for (color, path) in &outlines {
        ctx.set_stroke_style_str(color);
        ctx.stroke_with_path(path);
    }
    ctx.restore();
}

/// Faithful port of the reference `DrawWorld` text layout, drawn in
/// **state-batched passes**: the disc/zone/outline geometry comes from the
/// cached dot paths (`draw_world_dots`), and here every glyph kind (hex#,
/// starport, gas giant, bases, UWP, allegiance, name) is drawn as one pass that
/// sets the canvas font/fill/align **once** then loops `fillText` over all
/// on-screen worlds — instead of re-setting that state per glyph per world.
/// Offsets and font sizes are in parsec units (× scale → px); `cs = s ·
/// CONTENT_SCALE` sizes glyphs to fill the hex while layout offsets use true `s`.
pub(crate) fn draw_world_glyphs(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, sectors: &[&SectorData]) {
    let ctx = &canvas.ctx;
    let s = view.scale;
    let poster = s >= WORLD_FULL_SCALE; // poster vs atlas positions
    let show_uwp = s >= WORLD_UWP_SCALE;
    let cs = s * CONTENT_SCALE;

    // Layout offsets (parsec), poster vs atlas (RenderContext / Stylesheet).
    let (sp_y, uwp_y, name_y) = if poster { (-0.225, 0.225, 0.37) } else { (-0.24, 0.24, 0.40) };
    let (gg_x, gg_y) = if poster { (0.25, -0.18) } else { (0.225, -0.125) };
    let base_x = if poster { -0.25 } else { -0.225 };
    let zone_r = 0.4 * s; // (only used to size the off-screen cull margin)

    // Font sizes (parsec → px), porting Stylesheet's fontScale.
    let font_scale = if s <= 96.0 { 1.0 } else { 96.0 / s.min(192.0) };
    let name_pt = (if poster { 0.15 * font_scale } else { 0.2 }) * cs;
    let uwp_pt = 0.13 * font_scale * cs;
    let hex_pt = 0.10 * font_scale * cs;
    let name_font = format!("700 {}px {DEFAULT_FONT}", name_pt.max(7.0) as i32);
    let uwp_font = format!("500 {}px {DEFAULT_FONT}", uwp_pt.max(7.0) as i32);
    let hex_font = format!("{}px {DEFAULT_FONT}", hex_pt.max(6.0) as i32);
    let glyph_pt = (if poster { 0.15 * font_scale } else { 0.175 }) * cs;
    let glyph_font = format!("{}px {DEFAULT_FONT}", glyph_pt.max(7.0) as i32);
    // Base slots (left side); bottom slot rises when the UWP needs the room.
    let base_top_y = if poster { -0.18 } else { -0.125 };
    let base_bottom_y = if show_uwp { 0.1 } else if poster { 0.18 } else { 0.125 };

    let pad = zone_r + name_pt * 3.0 + 12.0;

    // Collect on-screen worlds once (screen coords), shared by every pass.
    let mut vis: Vec<(&World, f64, f64)> = Vec::new();
    for sector in sectors {
        let Some(loc) = sector.info.location else { continue };
        for world in &sector.worlds {
            let Some((col, row)) = parse_hex(&world.hex) else { continue };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (x, y) = view.to_screen(w, h, hex_parsec(wc, wr));
            if !on_screen(x, y, w, h, pad) {
                continue;
            }
            vis.push((world, x, y));
        }
    }
    if vis.is_empty() {
        return;
    }

    // Text is centered vertically at its y; set once for every pass below.
    ctx.set_text_baseline("middle");

    // ── Hex number (top, just inside the top edge — reference TopCenter).
    ctx.set_font(&hex_font);
    ctx.set_text_align("center");
    ctx.set_fill_style_str("#9aa3b8");
    let hex_dy = -0.5 * s + hex_pt * 0.55;
    for (world, x, y) in &vis {
        let _ = ctx.fill_text(&world.hex, *x, *y + hex_dy);
    }

    // ── Starport class (above the disc). Same font as names (700, name_pt).
    ctx.set_font(&name_font);
    ctx.set_fill_style_str("#e9eef9");
    for (world, x, y) in &vis {
        if let Some(sp) = world.uwp.chars().next() {
            if sp != '?' {
                let _ = ctx.fill_text(sp.encode_utf8(&mut [0u8; 4]), *x, *y + sp_y * s);
            }
        }
    }

    // ── Gas giant (upper-right): filled discs batched into one path; Saturn
    // ring (only when zoomed past the UWP threshold) stroked per giant.
    {
        let r = (0.05 * cs).max(1.0);
        let disc = Path2d::new().unwrap();
        let mut any = false;
        let has_gg = |wld: &World| wld.pbg.as_bytes().get(2).is_some_and(|&b| b > b'0' && b != b'?');
        for (world, x, y) in &vis {
            if has_gg(world) {
                let (gx, gy) = (*x + gg_x * s, *y + gg_y * s);
                disc.move_to(gx + r, gy);
                let _ = disc.arc(gx, gy, r, 0.0, std::f64::consts::TAU);
                any = true;
            }
        }
        if any {
            ctx.set_fill_style_str("#cfd6e6");
            ctx.fill_with_path_2d(&disc);
            if show_uwp {
                ctx.set_stroke_style_str("#cfd6e6");
                ctx.set_line_width((r / 4.0).max(0.6));
                for (world, x, y) in &vis {
                    if has_gg(world) {
                        let (gx, gy) = (*x + gg_x * s, *y + gg_y * s);
                        ctx.begin_path();
                        let _ = ctx.ellipse(gx, gy, r * 1.75, r * 0.4, -0.5236, 0.0, std::f64::consts::TAU);
                        ctx.stroke();
                    }
                }
            }
        }
    }

    // ── Bases (left side) as classic glyphs. Font/align set once; fill toggles
    // only between white and red (red is sparse), so track the last color.
    ctx.set_font(&glyph_font);
    ctx.set_text_align("center");
    let mut last = "";
    let bx = base_x * s;
    for (world, x, y) in &vis {
        let mut chars = world.bases.chars();
        let mut bottom_used = false;
        if let Some(c0) = chars.next() {
            if let Some(g) = glyph::base_glyph(&world.allegiance, c0) {
                bottom_used = g.bias == glyph::Bias::Bottom;
                let col = if g.highlight { C_RED } else { "#e9eef9" };
                if col != last { ctx.set_fill_style_str(col); last = col; }
                let gy = if bottom_used { base_bottom_y } else { base_top_y } * s;
                let _ = ctx.fill_text(g.chars, *x + bx, *y + gy);
            }
        }
        if let Some(c1) = chars.next() {
            if let Some(g) = glyph::base_glyph(&world.allegiance, c1) {
                let bottom = !bottom_used;
                let col = if g.highlight { C_RED } else { "#e9eef9" };
                if col != last { ctx.set_fill_style_str(col); last = col; }
                let gy = if bottom { base_bottom_y } else { base_top_y } * s;
                let _ = ctx.fill_text(g.chars, *x + bx, *y + gy);
            }
        }
    }

    // ── UWP (above name), only past the UWP scale threshold.
    if show_uwp {
        ctx.set_font(&uwp_font);
        ctx.set_text_align("center");
        ctx.set_fill_style_str("#c9d2e4");
        for (world, x, y) in &vis {
            if is_placeholder(world) {
                continue; // no "???????-?" line — the glyph stands in for it
            }
            let _ = ctx.fill_text(&world.uwp, *x, *y + uwp_y * s);
        }
    }

    // ── Allegiance code (e.g. NaHu) to the right of the disc, when zoomed in.
    if s >= ALLEGIANCE_MIN_SCALE {
        ctx.set_font(&uwp_font);
        ctx.set_text_align("left");
        ctx.set_fill_style_str("#aab3c8");
        for (world, x, y) in &vis {
            if is_placeholder(world) {
                continue;
            }
            if !world.allegiance.is_empty() && world.allegiance != "--" {
                let _ = ctx.fill_text(&world.allegiance, *x + 0.20 * s, *y + 0.08 * s);
            }
        }
    }

    // ── World name (bottom). High-pop (≥1e9) in ALL CAPS, capitals in red —
    // the reference's `IsHi` uppercase + capital highlight. Fill toggles only
    // for the sparse capitals, so track the last color rather than re-setting.
    ctx.set_font(&name_font);
    ctx.set_text_align("center");
    let mut last = "";
    let name_dy = name_y * s;
    for (world, x, y) in &vis {
        let hi_pop = world.uwp.as_bytes().get(4).copied().and_then(ehex).is_some_and(|p| p >= 9);
        let is_capital = world.codes().any(|c| matches!(c, "Cp" | "Cs" | "Cx" | "Capital"));
        let col = if is_capital { "#e8636f" } else { "#e9eef9" };
        if col != last { ctx.set_fill_style_str(col); last = col; }
        if hi_pop {
            let _ = ctx.fill_text(&world.name.to_uppercase(), *x, *y + name_dy);
        } else {
            let _ = ctx.fill_text(&world.name, *x, *y + name_dy);
        }
    }
}

/// Hex (ehex) digit value: 0-9, A=10 … (Traveller extended hex).
fn ehex(c: u8) -> Option<i32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as i32),
        b'A'..=b'Z' => Some((c - b'A') as i32 + 10),
        _ => None,
    }
}

/// World disc (fill, optional outline), porting `Stylesheet.WorldColors`
/// detail-color mode (color by trade classification).
fn world_colors(world: &World, more_colors: bool) -> (&'static str, Option<&'static str>) {
    let has = |code: &str| world.codes().any(|c| c == code);
    let atmo = world.uwp.as_bytes().get(2).copied().and_then(ehex);
    let hydro = world.uwp.as_bytes().get(3).copied().and_then(ehex);
    if !more_colors {
        // Plain mode: water worlds blue, everything else white (no trade-class
        // tints) — the reference's "More World Colors" off.
        let water = hydro.is_some_and(|h| h > 0)
            && atmo.is_some_and(|a| (2..=9).contains(&a) || (13..=15).contains(&a));
        let vacuum = has("Va") || atmo == Some(0);
        return if vacuum {
            ("#000000", Some("#ffffff"))
        } else if water {
            (C_WATER, None)
        } else {
            (C_DRY, None)
        };
    }
    let (ag, ri, ind) = (has("Ag"), has("Ri"), has("In"));
    let vacuum = has("Va") || atmo == Some(0);
    let water = hydro.is_some_and(|h| h > 0)
        && atmo.is_some_and(|a| (2..=9).contains(&a) || (13..=15).contains(&a));

    if ag && ri {
        (C_AMBER, None)
    } else if ag {
        ("#048104", None) // Green
    } else if ri {
        ("#a000a0", None) // Purple (Rich)
    } else if ind {
        ("#888888", None) // Gray (Industrial)
    } else if atmo.is_some_and(|a| a > 10) {
        ("#cc6626", None) // Rust (dense/exotic atmosphere)
    } else if vacuum {
        ("#000000", Some("#ffffff")) // Black disc, white outline
    } else if water {
        (C_WATER, None) // DeepSkyBlue
    } else {
        (C_DRY, None) // White
    }
}
