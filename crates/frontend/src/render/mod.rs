//! Map rendering — scene logic only, expressed against `trait Canvas`.
//!
//! Phase 6: OTU styling (`Stylesheet` palette) + LOD detail tiers. Macro view
//! gets red polity borders, white dashed routes, region labels and a star
//! field; the close view colors worlds (water/dry, amber/red zones) and reveals
//! names then UWPs as you zoom in. Absolute parsec coordinates throughout. This
//! module knows nothing about `web-sys` beyond the `Canvas2d` backend handoff.
//!
//! The frame is split into per-pass submodules (one file per render layer);
//! `draw` below is the only orchestrator. Shared view/transform math, the LOD
//! thresholds, the palette, and the hex geometry helpers live in [`common`].

mod borders;
mod common;
mod grid;
mod hud;
mod labels;
mod overlays;
mod routes;
mod stars;
mod status;
mod worlds;

use std::collections::HashMap;

use tmap_core::dto::{Overlays, RouteResult, SectorData};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::canvas::Canvas;

// Public API (unchanged for `main.rs`).
pub use common::{
    fit_sector, home_view, sector_hex_parsec, visible_sectors, world_to_parsec, RenderOptions,
    ViewState, MAX_SCALE, MIN_SCALE, WORLD_MIN_SCALE,
};

use common::{
    MACRO_MAX_SCALE, MACRO_WORLDS_MAX, MACRO_WORLDS_MIN, PARSEC_GRID_MIN_SCALE, ROUTE_MIN_SCALE,
    SECTOR_GRID_MIN, SECTOR_H, SECTOR_NAME_MAX, SECTOR_NAME_MIN, SECTOR_W, SUBSECTOR_GRID_MIN,
    SUBSECTOR_H, SUBSECTOR_NAME_MAX, SUBSECTOR_NAME_MIN, SUBSECTOR_W, WORLD_BASIC_SCALE,
};

use crate::canvas::Canvas2d;

/// Clear all cached per-sector geometry (call on a milieu switch, when the world
/// data underneath the caches changes). Wired up by the upcoming milieu selector.
#[allow(dead_code)]
pub fn clear_caches() {
    grid::clear_grid_geom();
    borders::clear_border_caches();
    worlds::clear_sector_dots();
}

/// Draw the map under the current view, choosing layers by LOD.
pub fn draw(
    canvas: &HtmlCanvasElement,
    sectors: &[&SectorData],
    overlays: Option<&Overlays>,
    sector_index: &HashMap<(i32, i32), String>,
    view: ViewState,
    opts: RenderOptions,
    route: Option<&RouteResult>,
) {
    let Some(ctx) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|o| o.dyn_into::<CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    // Draw in logical (CSS) pixels: scale the context by the device pixel ratio
    // so `view.scale` is px-per-parsec in CSS units — matching the reference's
    // `Stylesheet` calibration (fontScale, LOD thresholds) and staying crisp on
    // retina. The 2D context persists between calls, so reset the transform.
    let (buf_w, css_w) = (canvas.width() as f64, canvas.client_width().max(1) as f64);
    let dpr = buf_w / css_w;
    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    let _ = ctx.scale(dpr, dpr);
    let w = css_w;
    let h = canvas.client_height().max(1) as f64;
    let c = Canvas2d { ctx };

    // Per-layer timing for the profiling HUD. `now()` (performance.now) is
    // cheap, so always measure; only paint the overlay when `perf_hud` is on.
    let mut marks: Vec<(&'static str, f64)> = Vec::new();
    let mut t = now();
    let mut mark = |label: &'static str, marks: &mut Vec<(&'static str, f64)>| {
        let n = now();
        marks.push((label, n - t));
        t = n;
    };

    c.clear("#000000", w, h);
    // Galaxy image behind the starfield (macro zoom only; fades out by scale 2).
    stars::draw_galaxy(&c, &view, w, h);
    stars::draw_stars(&c, &view, w, h);
    mark("stars", &mut marks);

    if view.scale < MACRO_MAX_SCALE {
        if let Some(ov) = overlays {
            overlays::draw_overlays(&c, &view, w, h, ov, opts);
        }
    }
    // Capitals + homeworlds (Worlds.xml) over the macro view (scale 0.5–4).
    if opts.important_worlds && (MACRO_WORLDS_MIN..=MACRO_WORLDS_MAX).contains(&view.scale) {
        if let Some(ov) = overlays {
            overlays::draw_world_labels(&c, &view, w, h, ov);
        }
    }
    mark("macro", &mut marks);

    // Sector / subsector boundary grids and background names.
    if opts.sector_grid && view.scale >= SUBSECTOR_GRID_MIN {
        grid::draw_grid_lines(&c, &view, w, h, SUBSECTOR_W, SUBSECTOR_H, "rgba(140,160,200,0.34)", 1.2);
    }
    if opts.sector_grid && view.scale >= SECTOR_GRID_MIN {
        grid::draw_grid_lines(&c, &view, w, h, SECTOR_W, SECTOR_H, "rgba(170,190,225,0.55)", 1.4);
    }
    if opts.sector_names && (SECTOR_NAME_MIN..=SECTOR_NAME_MAX).contains(&view.scale) {
        labels::draw_sector_names(&c, &view, w, h, sector_index);
    }
    if opts.region_names && (SUBSECTOR_NAME_MIN..=SUBSECTOR_NAME_MAX).contains(&view.scale) {
        for sector in sectors {
            labels::draw_subsector_names(&c, &view, w, h, sector);
        }
    }
    mark("grid+names", &mut marks);

    if view.scale >= WORLD_MIN_SCALE {
        // Micro borders (fill behind everything, then stroke).
        if opts.borders {
            borders::draw_micro_borders(&c, &view, w, h, dpr, sectors, opts.filled_borders);
        }
        mark("borders", &mut marks);
        if opts.routes && view.scale >= ROUTE_MIN_SCALE {
            for sector in sectors {
                routes::draw_routes(&c, &view, w, h, sector);
            }
        }
        // Per-parsec hex grid only once hexes are big enough to read (and to
        // avoid drawing tens of thousands of hexagons when zoomed out).
        if opts.sector_grid && view.scale >= PARSEC_GRID_MIN_SCALE {
            grid::draw_hex_grid(&c, &view, w, h, dpr, sector_index);
        }
        mark("routes+hexgrid", &mut marks);
        // Disc / zone-ring / vacuum-outline layer: identical geometry at every
        // detail scale (the reference's in-hex disc), so always drawn from the
        // batched, cached per-sector dot paths — a few fills, not a call/world.
        worlds::draw_world_dots(&c, &view, w, h, dpr, sectors, opts.more_world_colors);
        // Placeholder (`*`) / anomaly (`⌖`) glyphs stand in for the disc on
        // unknown-UWP worlds and deep-space stations.
        worlds::draw_placeholder_glyphs(&c, &view, w, h, sectors);
        // At basic scale and up, add the per-world text + small glyphs (hex#,
        // starport, gas giant, bases, UWP, allegiance, name) in state-batched
        // passes: canvas font/fill/align set once per pass, not once per glyph.
        if view.scale >= WORLD_BASIC_SCALE {
            worlds::draw_world_glyphs(&c, &view, w, h, sectors);
        }
        mark("worlds", &mut marks);
        // Border labels ("Third Imperium") once names are legible.
        if opts.region_names && view.scale >= 16.0 {
            for sector in sectors {
                labels::draw_border_labels(&c, &view, w, h, sector);
            }
        }
    }

    // Dim sectors not flagged Official/Preserve/InReview (opt-in appearance).
    if opts.dim_unofficial {
        status::draw_dim_overlay(&c, &view, w, h, sectors);
    }

    // A computed jump route (from `/api/route`), highlighted over the map.
    if let Some(r) = route {
        routes::draw_jump_route(&c, &view, w, h, r);
    }

    // Compass labels last, on top of everything, at every zoom.
    if opts.galactic_direction {
        labels::draw_galactic_directions(&c, w, h);
    }

    mark("labels+misc", &mut marks);

    if opts.perf_hud {
        hud::draw_perf_hud(&c, w, h, &marks, sectors.len(), view.scale);
    }
}

/// `performance.now()` in milliseconds (monotonic, sub-ms). Returns 0 if
/// unavailable (e.g. no window) so timing degrades gracefully.
pub(crate) fn now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}
