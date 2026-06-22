//! Detail-tier world rendering: the cached disc/zone/outline "dot" layer, and
//! the state-batched glyph passes (hex#, starport, gas giant, bases, UWP,
//! allegiance, name) — a faithful port of the reference `DrawWorld` layout.

use std::cell::RefCell;
use std::collections::HashMap;

use tmap_core::astrometrics::parse_hex;
use tmap_core::dto::{SectorData, World};

use crate::canvas::{Affine, Canvas, Geometry, PathBuilder, Shadow, StrokeStyle, TextAlign};
use crate::glyph;

use super::common::{
    hex_parsec, hex_vertex_r, on_screen, sector_in_viewport, world_hex, ViewState,
    ALLEGIANCE_MIN_SCALE, CONTENT_SCALE, HEX_VR, WORLD_BASIC_SCALE, WORLD_FULL_SCALE,
    WORLD_UWP_SCALE,
};
use super::Theme;

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
    discs: Vec<(String, Geometry)>,
    outlines: Vec<(String, Geometry)>,
    zones: Vec<(String, Geometry)>,
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
pub(crate) fn draw_placeholder_glyphs(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sectors: &[&SectorData],
    theme: &Theme,
) {
    let s = view.scale;
    // `*`/`⌖` are `WorldDetails.Type` glyphs — Atlas+ only. Below that (Dotmap
    // zoom) non-anomaly placeholders render as plain dots (`build_sector_dots`).
    if s < WORLD_BASIC_SCALE {
        return;
    }
    let cs = s * CONTENT_SCALE;
    // Reference FontInfo sizes are 0.6 parsec (same convention as our other
    // glyph fonts, ~size·cs px).
    let star_font = format!(
        "{}px Georgia, 'Times New Roman', serif",
        (0.6 * cs).max(6.0) as i32
    );
    let anomaly_font = format!(
        "{}px 'Arial Unicode MS', 'Segoe UI Symbol', sans-serif",
        (0.6 * cs).max(6.0) as i32
    );
    for sector in sectors {
        let Some(loc) = sector.info.location else {
            continue;
        };
        for world in &sector.worlds {
            if !is_placeholder(world) {
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (x, y) = view.to_screen(w, h, hex_parsec(wc, wr));
            if !on_screen(x, y, w, h, cs) {
                continue;
            }
            if is_anomaly(world) {
                // position (0, 0)
                c.fill_text(
                    "\u{2316}",
                    x,
                    y,
                    theme.highlight,
                    &anomaly_font,
                    TextAlign::Center,
                );
            } else {
                // position (0, 0.17)
                c.fill_text(
                    "*",
                    x,
                    y + 0.17 * s,
                    theme.world_dry,
                    &star_font,
                    TextAlign::Center,
                );
            }
        }
    }
}

fn build_sector_dots(
    sector: &SectorData,
    more_colors: bool,
    dotmap: bool,
    theme: &Theme,
) -> SectorDots {
    use std::f64::consts::{PI, TAU};
    let disc_r = disc_radius(dotmap);
    let mut discs: HashMap<String, PathBuilder> = HashMap::new();
    let mut outlines: HashMap<String, PathBuilder> = HashMap::new();
    let mut zones: HashMap<String, PathBuilder> = HashMap::new();
    if let Some(loc) = sector.info.location {
        let add_circle = |map: &mut HashMap<String, PathBuilder>, color: &str, cx: f64, cy: f64| {
            let p = map.entry(color.to_owned()).or_default();
            p.move_to(cx + disc_r, cy);
            p.arc(cx, cy, disc_r, 0.0, TAU);
        };
        for world in &sector.worlds {
            // Placeholder worlds: at Atlas+ zoom they get the `*`/`⌖` glyph
            // (`draw_placeholder_glyphs`), so skip their dot geometry. But at
            // Dotmap zoom the reference draws every *non-anomaly* world —
            // placeholders included — as a plain white dot (`WorldDetails.Type`
            // is off below scale 24, so no glyph), giving the dense uniform dot
            // field of an uncharted sector. Anomalies draw nothing until Atlas.
            if is_placeholder(world) {
                if dotmap && !is_anomaly(world) {
                    let Some((col, row)) = parse_hex(&world.hex) else {
                        continue;
                    };
                    let (wc, wr) = world_hex(loc.x, loc.y, col, row);
                    let (cx, cy) = hex_parsec(wc, wr);
                    add_circle(&mut discs, theme.world_dry, cx, cy);
                }
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (cx, cy) = hex_parsec(wc, wr);
            if theme.zone_perimeters {
                // Mongoose: a hexagon outline at 0.95× around the hex, in the zone
                // color — green for every world by default, amber/red when zoned.
                let zcolor = match world.zone.as_str() {
                    "A" => theme.amber,
                    "R" => theme.red_zone,
                    _ => theme.green_zone.unwrap_or(theme.amber),
                };
                let p = zones.entry(zcolor.to_owned()).or_default();
                let v0 = hex_vertex_r(wc, wr, 0, HEX_VR * 0.95);
                p.move_to(v0.0, v0.1);
                for k in 1..6 {
                    let v = hex_vertex_r(wc, wr, k, HEX_VR * 0.95);
                    p.line_to(v.0, v.1);
                }
                p.close();
            } else {
                // Travel zone: a single open-bottom arc behind the disc (amber/red).
                let zc = match world.zone.as_str() {
                    "A" => Some(theme.amber),
                    "R" => Some(theme.red_zone),
                    _ => None,
                };
                if let Some(zc) = zc {
                    let (a0, a1) = (PI - 0.384, 2.0 * PI + 0.384);
                    let p = zones.entry(zc.to_owned()).or_default();
                    p.move_to(cx + ZONE_R * a0.cos(), cy + ZONE_R * a0.sin());
                    p.arc(cx, cy, ZONE_R, a0, a1);
                }
            }
            // Mongoose offsets the disc off hex center (DiscPosition); the zone
            // hexagon stays centered, the disc + its outline shift down-left.
            let (dcx, dcy) = if theme.mongoose_layout {
                (cx - 0.11, cy + 0.16)
            } else {
                (cx, cy)
            };
            let (fill, outline) = world_colors(world, more_colors, theme);
            add_circle(&mut discs, fill, dcx, dcy);
            if let Some(oc) = outline {
                add_circle(&mut outlines, oc, dcx, dcy);
            }
        }
    }
    let finish = |m: HashMap<String, PathBuilder>| -> Vec<(String, Geometry)> {
        m.into_iter().map(|(c, p)| (c, p.finish())).collect()
    };
    SectorDots {
        more_colors,
        dotmap,
        discs: finish(discs),
        outlines: finish(outlines),
        zones: finish(zones),
    }
}

/// Dot-tier worlds (scale < `WORLD_BASIC_SCALE`): batched discs + zone rings
/// from the per-sector cache, drawn under one view transform.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_world_dots(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    dpr: f64,
    sectors: &[&SectorData],
    more_colors: bool,
    theme: &Theme,
) {
    let s = view.scale;
    let dotmap = s < WORLD_BASIC_SCALE; // bigger discs when no per-world detail
    let mut discs: HashMap<String, PathBuilder> = HashMap::new();
    let mut outlines: HashMap<String, PathBuilder> = HashMap::new();
    let mut zones: HashMap<String, PathBuilder> = HashMap::new();
    let merge = |dst: &mut HashMap<String, PathBuilder>, src: &[(String, Geometry)]| {
        for (color, p) in src {
            dst.entry(color.clone()).or_default().add(p);
        }
    };
    SECTOR_DOTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        for sector in sectors {
            let Some(loc) = sector.info.location else {
                continue;
            };
            if !sector_in_viewport((loc.x, loc.y), view, w, h) {
                continue;
            }
            let dots = cache
                .entry((loc.x, loc.y))
                .or_insert_with(|| build_sector_dots(sector, more_colors, dotmap, theme));
            if dots.more_colors != more_colors || dots.dotmap != dotmap {
                *dots = build_sector_dots(sector, more_colors, dotmap, theme); // tier/colors changed
            }
            merge(&mut zones, &dots.zones);
            merge(&mut discs, &dots.discs);
            merge(&mut outlines, &dots.outlines);
        }
    });
    let m = Affine::scale_translate(
        dpr * s,
        dpr * (w / 2.0 - view.center.0 * s),
        dpr * (h / 2.0 - view.center.1 * s),
    );
    let cs = s * CONTENT_SCALE;
    // Zones first (behind), then disc fills, then vacuum outlines. Line widths
    // are css px ÷ s (the transform scales by s).
    let zone_style = StrokeStyle::plain(((0.03 * cs).max(1.5)) / s);
    for (color, p) in zones {
        c.stroke_geometry(&p.finish(), m, &color, &zone_style, None);
    }
    for (color, p) in discs {
        c.fill_geometry(&p.finish(), m, &color, 1.0);
    }
    let outline_style = StrokeStyle::plain(((0.02 * cs).max(1.0)) / s);
    for (color, p) in outlines {
        c.stroke_geometry(&p.finish(), m, &color, &outline_style, None);
    }
}

/// Faithful port of the reference `DrawWorld` text layout, drawn in
/// **state-batched passes**: the disc/zone/outline geometry comes from the
/// cached dot paths (`draw_world_dots`), and here every glyph kind (hex#,
/// starport, gas giant, bases, UWP, allegiance, name) is drawn as one pass that
/// sets the canvas font/fill/align **once** then loops `fillText` over all
/// on-screen worlds — instead of re-setting that state per glyph per world.
/// Offsets and font sizes are in parsec units (× scale → px); `cs = s ·
/// CONTENT_SCALE` sizes glyphs to fill the hex while layout offsets use true `s`.
pub(crate) fn draw_world_glyphs(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sectors: &[&SectorData],
    theme: &Theme,
) {
    let s = view.scale;
    let poster = s >= WORLD_FULL_SCALE; // poster vs atlas positions
    let show_uwp = s >= WORLD_UWP_SCALE;
    let cs = s * CONTENT_SCALE;

    // Layout offsets (parsec), poster vs atlas (RenderContext / Stylesheet).
    // Mongoose relays everything out around an off-center disc (Stylesheet.cs:1040-1047):
    // name at center, UWP below, starport upper-right, gas giant on top.
    let mongoose = theme.mongoose_layout;
    let (sp_x, sp_y, uwp_y, name_y) = if mongoose {
        (0.175, 0.17, 0.40, -0.04)
    } else if poster {
        (0.0, -0.225, 0.225, 0.37)
    } else {
        (0.0, -0.24, 0.24, 0.40)
    };
    let (gg_x, gg_y) = if mongoose {
        (0.0, -0.23)
    } else if poster {
        (0.25, -0.18)
    } else {
        (0.225, -0.125)
    };
    let base_x = if mongoose {
        -0.22
    } else if poster {
        -0.25
    } else {
        -0.225
    };
    let zone_r = 0.4 * s; // (only used to size the off-screen cull margin)

    // Font sizes (parsec → px), porting Stylesheet's fontScale.
    let font_scale = if s <= 96.0 { 1.0 } else { 96.0 / s.min(192.0) };
    let name_pt = (if poster { 0.15 * font_scale } else { 0.2 }) * cs;
    let uwp_pt = 0.13 * font_scale * cs;
    let hex_pt = 0.10 * font_scale * cs;
    let ff = theme.font;
    let name_font = format!("700 {}px {ff}", name_pt.max(7.0) as i32);
    let uwp_font = format!("500 {}px {ff}", uwp_pt.max(7.0) as i32);
    let hex_font = format!("{}px {ff}", hex_pt.max(6.0) as i32);
    let glyph_pt = (if poster { 0.15 * font_scale } else { 0.175 }) * cs;
    let glyph_font = format!("{}px {ff}", glyph_pt.max(7.0) as i32);
    // Base slots (left side); bottom slot rises when the UWP needs the room.
    let base_top_y = if poster { -0.18 } else { -0.125 };
    let base_bottom_y = if show_uwp {
        0.1
    } else if poster {
        0.18
    } else {
        0.125
    };

    let pad = zone_r + name_pt * 3.0 + 12.0;

    // Collect on-screen worlds once (screen coords), shared by every pass.
    let mut vis: Vec<(&World, f64, f64)> = Vec::new();
    for sector in sectors {
        let Some(loc) = sector.info.location else {
            continue;
        };
        for world in &sector.worlds {
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
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

    // ── Hex number (top, just inside the top edge — reference TopCenter).
    // Skipped when the style numbers *every* hex (Draft/FASA/Terminal) — that pass
    // already labels the world's hex (reference gates the per-world number the same).
    if !theme.number_all_hexes {
        let hex_dy = -0.5 * s + hex_pt * 0.55;
        for (world, x, y) in &vis {
            c.fill_text(
                &world.hex,
                *x,
                *y + hex_dy,
                theme.text_hex,
                &hex_font,
                TextAlign::Center,
            );
        }
    }

    // ── Starport class (above the disc). Same font as names (700, name_pt).
    if !theme.drop_starport {
        for (world, x, y) in &vis {
            if let Some(sp) = world.uwp.chars().next() {
                if sp != '?' {
                    c.fill_text(
                        sp.encode_utf8(&mut [0u8; 4]),
                        *x + sp_x * s,
                        *y + sp_y * s,
                        theme.text,
                        &name_font,
                        TextAlign::Center,
                    );
                }
            }
        }
    }

    // ── Gas giant (upper-right): a filled disc, plus a Saturn ring (only when
    // zoomed past the UWP threshold).
    if !theme.drop_gas_giant {
        let r = (0.05 * cs).max(1.0);
        let has_gg = |wld: &World| {
            wld.pbg
                .as_bytes()
                .get(2)
                .is_some_and(|&b| b > b'0' && b != b'?')
        };
        for (world, x, y) in &vis {
            if !has_gg(world) {
                continue;
            }
            let (gx, gy) = (*x + gg_x * s, *y + gg_y * s);
            c.fill_circle(gx, gy, r, theme.text_gg);
            if show_uwp {
                c.stroke_ellipse(
                    gx,
                    gy,
                    r * 1.75,
                    r * 0.4,
                    -std::f64::consts::FRAC_PI_6,
                    theme.text_gg,
                    (r / 4.0).max(0.6),
                );
            }
        }
    }

    // ── Bases (left side) as classic glyphs. `hi` is the highlight color unless
    // this theme drops highlights.
    let hi = if theme.drop_highlight {
        theme.text
    } else {
        theme.highlight
    };
    if !theme.drop_bases {
        let bx = base_x * s;
        for (world, x, y) in &vis {
            let mut chars = world.bases.chars();
            let mut bottom_used = false;
            if let Some(c0) = chars.next() {
                if let Some(g) = glyph::base_glyph(&world.allegiance, c0) {
                    bottom_used = g.bias == glyph::Bias::Bottom;
                    let col = if g.highlight { hi } else { theme.text };
                    let gy = if bottom_used {
                        base_bottom_y
                    } else {
                        base_top_y
                    } * s;
                    c.fill_text(
                        g.chars,
                        *x + bx,
                        *y + gy,
                        col,
                        &glyph_font,
                        TextAlign::Center,
                    );
                }
            }
            if let Some(c1) = chars.next() {
                if let Some(g) = glyph::base_glyph(&world.allegiance, c1) {
                    let bottom = !bottom_used;
                    let col = if g.highlight { hi } else { theme.text };
                    let gy = if bottom { base_bottom_y } else { base_top_y } * s;
                    c.fill_text(
                        g.chars,
                        *x + bx,
                        *y + gy,
                        col,
                        &glyph_font,
                        TextAlign::Center,
                    );
                }
            }
        }
    } // end !drop_bases

    // ── UWP (above name), only past the UWP scale threshold.
    if show_uwp && !theme.drop_uwp {
        let pad = (uwp_pt * 0.35).max(1.0);
        for (world, x, y) in &vis {
            if is_placeholder(world) {
                continue; // no "???????-?" line — the glyph stands in for it
            }
            let ty = *y + uwp_y * s;
            // Mongoose: white UWP on a solid black box (textBackgroundStyle=Filled).
            // (web-sys `measure_text` isn't in our feature set; the UWP is a fixed
            // ~9-char string, so estimate the box from the glyph count × font width.)
            if theme.uwp_filled {
                let bw = world.uwp.chars().count() as f64 * uwp_pt * 0.62 + pad * 2.0;
                let bh = uwp_pt + pad;
                c.fill_rect(*x - bw / 2.0, ty - bh / 2.0, bw, bh, "#000000");
            }
            c.fill_text(
                &world.uwp,
                *x,
                ty,
                theme.text_uwp,
                &uwp_font,
                TextAlign::Center,
            );
        }
    }

    // ── Allegiance code (e.g. NaHu) to the right of the disc, when zoomed in.
    if s >= ALLEGIANCE_MIN_SCALE && !theme.drop_allegiance {
        for (world, x, y) in &vis {
            if is_placeholder(world) {
                continue;
            }
            if !world.allegiance.is_empty() && world.allegiance != "--" {
                c.fill_text(
                    &world.allegiance,
                    *x + 0.20 * s,
                    *y + 0.08 * s,
                    theme.text_alleg,
                    &uwp_font,
                    TextAlign::Left,
                );
            }
        }
    }

    // ── World name (bottom). High-pop (≥1e9) in ALL CAPS, capitals in red —
    // the reference's `IsHi` uppercase + capital highlight.
    let name_dy = name_y * s;
    // Candy squishes names vertically (`worlds.textStyle.Scale (1.0, 0.5)`); other
    // styles draw 1:1, so skip the rotate/scale path unless asked.
    let (nsx, nsy) = theme.world_name_scale;
    let squish = (nsx - 1.0).abs() > f64::EPSILON || (nsy - 1.0).abs() > f64::EPSILON;
    for (world, x, y) in &vis {
        let hi_pop = world
            .uwp
            .as_bytes()
            .get(4)
            .copied()
            .and_then(ehex)
            .is_some_and(|p| p >= 9);
        let is_capital = world
            .codes()
            .any(|c| matches!(c, "Cp" | "Cs" | "Cx" | "Capital"));
        let col = if is_capital && !theme.drop_highlight {
            theme.capital
        } else {
            theme.text
        };
        let name = if hi_pop || theme.uppercase_worlds {
            world.name.to_uppercase()
        } else {
            world.name.clone()
        };
        if squish {
            c.fill_text_rotated(
                &name,
                *x,
                *y + name_dy,
                col,
                &name_font,
                0.0,
                nsx,
                nsy,
                TextAlign::Center,
                None,
            );
        } else {
            c.fill_text(&name, *x, *y + name_dy, col, &name_font, TextAlign::Center);
        }
    }
}

/// **Candy** world rendering — the reference `useWorldImages` ("Eye-Candy")
/// branch of `DrawWorld` (`RenderContext.cs:1356-1481`). Replaces the colored disc
/// with a hydrographics globe texture (`res/Candy/Hyd*`/`Belt`), and lays the
/// decorations out to the **right** of the globe on a growing `decorationRadius`
/// ring: a 4-arc near-full zone circle, the gas-giant marker, the UWP, then the
/// (vertically-squished, left-aligned) name — not stacked below the disc.
pub(crate) fn draw_world_images(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sectors: &[&SectorData],
    theme: &Theme,
) {
    use std::f64::consts::PI;
    let s = view.scale;
    let show_uwp = s >= WORLD_UWP_SCALE;
    let cs = s * CONTENT_SCALE;
    let font_scale = if s <= 96.0 { 1.0 } else { 96.0 / s.min(192.0) };
    let ff = theme.font;
    let name_font = format!("700 {}px {ff}", (0.15 * font_scale * cs).max(7.0) as i32);
    let uwp_font = format!("500 {}px {ff}", (0.13 * font_scale * cs).max(7.0) as i32);
    let (nsx, nsy) = theme.world_name_scale;

    // Globe (≤0.3 parsec) + a name out to its right → generous right/vertical pad.
    let pad = 0.6 * s + (0.15 * cs).max(7.0) * 8.0 + 12.0;
    let mut vis: Vec<(&World, f64, f64)> = Vec::new();
    for sector in sectors {
        let Some(loc) = sector.info.location else {
            continue;
        };
        for world in &sector.worlds {
            if is_placeholder(world) {
                continue; // drawn by draw_placeholder_glyphs
            }
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
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

    for (world, x, y) in &vis {
        let b = world.uwp.as_bytes();
        let size = b.get(1).copied().and_then(ehex).unwrap_or(0);
        let hyd = b.get(3).copied().and_then(ehex).unwrap_or(0);

        // imageRadius (parsec): belt 0.3, else 0.3·(Size/5 + 0.2) / 2 (RenderContext:1360).
        let image_radius = (if size <= 0 {
            0.6
        } else {
            0.3 * (size as f64 / 5.0 + 0.2)
        }) / 2.0;

        if size <= 0 {
            // Belt: 1.5×1.0 aspect (RenderContext:1375-1379).
            let (rw, rh) = (image_radius * 1.5 * s, image_radius * s);
            c.draw_image(
                "/api/res/Candy/Belt.png",
                *x - rw,
                *y - rh,
                rw * 2.0,
                rh * 2.0,
                1.0,
            );
        } else {
            let n = if (1..=10).contains(&hyd) { hyd } else { 0 };
            let digit = if n == 10 {
                "A".to_string()
            } else {
                n.to_string()
            };
            let r = image_radius * s;
            c.draw_image(
                &format!("/api/res/Candy/Hyd{digit}.png"),
                *x - r,
                *y - r,
                r * 2.0,
                r * 2.0,
                1.0,
            );
        }

        let mut deco = image_radius + 0.1; // decorationRadius (RenderContext:1417)

        // ── Zone: four 80° arcs → a dashed near-full circle (RenderContext:1427-1430).
        let (amber, red) = (world.zone == "A", world.zone == "R");
        if amber || red {
            let zcolor = if amber { theme.amber } else { theme.red_zone };
            let zw = (0.035 * s).max(1.0);
            let r = deco * s;
            for start in [5.0_f64, 95.0, 185.0, 275.0] {
                c.stroke_arc(
                    *x,
                    *y,
                    r,
                    start * PI / 180.0,
                    (start + 80.0) * PI / 180.0,
                    zcolor,
                    zw,
                );
            }
            deco += 0.1;
        }

        // ── Gas giant: small highlight disc riding the ring, to the right (RenderContext:1437-1447).
        let has_gg = world
            .pbg
            .as_bytes()
            .get(2)
            .is_some_and(|&c| c > b'0' && c != b'?');
        if has_gg && !theme.drop_gas_giant {
            let gr = (0.05 * s).max(1.5);
            deco += 0.05;
            let gx = *x + deco * s;
            c.fill_circle(gx, *y, gr, theme.highlight);
            // Saturn ring: thin ellipse rotated −30° (RenderContext `DrawGasGiant`).
            c.stroke_ellipse(
                gx,
                *y,
                gr * 1.75,
                gr * 0.4,
                -PI / 6.0,
                theme.highlight,
                (gr / 4.0).max(0.6),
            );
            deco += 0.1;
        }

        // ── UWP (right of the ring, left-aligned) once past the UWP scale.
        if show_uwp && !theme.drop_uwp {
            c.fill_text(
                &world.uwp,
                *x + deco * s,
                *y - 0.18 * s,
                theme.text_uwp,
                &uwp_font,
                TextAlign::Left,
            );
        }

        // ── Name (right of the ring, left-aligned, vertically squished). Candy
        // draws names with a hard drop shadow (`textBackgroundStyle = Shadow`) for
        // the 3-D "eye-candy" look — a hard offset copy in the background color.
        let hi_pop = b.get(4).copied().and_then(ehex).is_some_and(|p| p >= 9);
        let is_capital = world
            .codes()
            .any(|c| matches!(c, "Cp" | "Cs" | "Cx" | "Capital"));
        let name = if hi_pop || theme.uppercase_worlds {
            world.name.to_uppercase()
        } else {
            world.name.clone()
        };
        let col = if is_capital && !theme.drop_highlight {
            theme.capital
        } else {
            theme.text
        };
        let shadow = theme.text_shadow.then(|| Shadow {
            color: theme.background.to_string(),
            dx: 2.0,
            dy: 2.0,
            blur: 1.0,
        });
        c.fill_text_rotated(
            &name,
            *x + deco * s,
            *y,
            col,
            &name_font,
            0.0,
            nsx,
            nsy,
            TextAlign::Left,
            shadow.as_ref(),
        );
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
fn world_colors(
    world: &World,
    more_colors: bool,
    theme: &Theme,
) -> (&'static str, Option<&'static str>) {
    let has = |code: &str| world.codes().any(|c| c == code);
    let atmo = world.uwp.as_bytes().get(2).copied().and_then(ehex);
    let hydro = world.uwp.as_bytes().get(3).copied().and_then(ehex);
    // Plain mode: water worlds blue, everything else the dry color (no trade-class
    // tints). The theme can force this (`showWorldDetailColors = false` — Atlas/FASA)
    // regardless of the user's "More World Colors" toggle.
    if !more_colors || theme.force_plain_worlds {
        let water = hydro.is_some_and(|h| h > 0)
            && atmo.is_some_and(|a| (2..=9).contains(&a) || (13..=15).contains(&a));
        let vacuum = has("Va") || atmo == Some(0);
        return if vacuum {
            (theme.vacuum_fill, Some(theme.world_dry))
        } else if water {
            (theme.world_water, theme.world_dry_outline)
        } else {
            (theme.world_dry, theme.world_dry_outline)
        };
    }
    let (ag, ri, ind) = (has("Ag"), has("Ri"), has("In"));
    let vacuum = has("Va") || atmo == Some(0);
    let water = hydro.is_some_and(|h| h > 0)
        && atmo.is_some_and(|a| (2..=9).contains(&a) || (13..=15).contains(&a));

    if ag && ri {
        (theme.amber, None)
    } else if ag {
        (theme.ag_green, None) // Green
    } else if ri {
        (theme.rich_purple, None) // Purple (Rich)
    } else if ind {
        (theme.ind_gray, None) // Gray (Industrial)
    } else if atmo.is_some_and(|a| a > 10) {
        (theme.exotic_rust, None) // Rust (dense/exotic atmosphere)
    } else if vacuum {
        (theme.vacuum_fill, Some(theme.world_dry)) // Black disc, white outline
    } else if water {
        (theme.world_water, theme.world_dry_outline) // DeepSkyBlue
    } else {
        (theme.world_dry, theme.world_dry_outline) // White
    }
}
