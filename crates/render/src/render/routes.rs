//! Micro (per-sector) trade routes — green polylines between hex endpoints.

use std::cell::RefCell;

use tmap_core::astrometrics::parse_hex;
use tmap_core::dto::{RouteResult, SectorData};

use crate::canvas::{Affine, Canvas, Geometry, PathBuilder, StrokeStyle};

use super::common::{hex_parsec, world_hex, ViewState};

/// Route stroke color, ported from `res/styles/otu.css` `route.<allegiance>`
/// rules (the reference applies the route's allegiance, defaulting to `"Im"`),
/// so the Imperial X-boat network is green and other polities use their hues.
fn route_color(allegiance: &str) -> &'static str {
    match allegiance {
        "Im" | "SoCf" => "#048104",          // green
        "As" | "AsXX" => "#ffff00",          // yellow
        "HvFd" | "Kk" | "KkTw" => "#808080", // gray
        "JuPr" => "#90ee90",                 // lightgreen
        "ZhCo" | "JAOz" | "JAsi" | "JCoK" | "JHhk" | "JLum" | "JMen" | "JPSt" | "JRar" | "JUkh"
        | "JVug" | "JuHl" | "JuRu" => "#add8e6", // lightblue
        _ => "#048104",                      // default allegiance "Im" → green
    }
}

/// Shorten a segment by `off` parsec at each end so a route stops short of the
/// world glyph instead of running into the disc (reference `OffsetSegment` /
/// `routeEndAdjust = 0.25`). Points are already in geometric (x-compressed) space,
/// so the offset applies directly.
fn offset_segment(a: (f64, f64), b: (f64, f64), off: f64) -> ((f64, f64), (f64, f64)) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return (a, b);
    }
    let (ddx, ddy) = (dx * off / len, dy * off / len);
    ((a.0 + ddx, a.1 + ddy), (b.0 - ddx, b.1 - ddy))
}

/// `routeEndAdjust` (parsec) — how far a route stops short of each world.
const ROUTE_END_ADJUST: f64 = 0.25;

/// Cached, **batched** route geometry: one world-space `Geometry` per stroke
/// color, holding every route segment of that color across all visible sectors
/// as sub-paths. Rebuilt only when the visible-route set (or the theme's micro-
/// route override) changes — so a steady pan strokes a handful of cached paths
/// instead of issuing one `stroke` per route segment every frame. The geometry
/// is world-space (the per-frame view transform is applied at draw), so it's
/// valid across pan/zoom; only the scale-dependent pen width is recomputed.
struct RouteCache {
    key: u64,
    buckets: Vec<(String, Geometry)>,
}

thread_local! {
    static ROUTE_CACHE: RefCell<Option<RouteCache>> = const { RefCell::new(None) };
}

/// Clear the cached route geometry (milieu switch / theme switch).
pub(crate) fn clear_route_cache() {
    ROUTE_CACHE.with(|c| *c.borrow_mut() = None);
}

/// Hash of the sectors that carry routes (+ the micro-route override), so the
/// batched geometry rebuilds only when the on-screen route set or forced color
/// changes — not on every pan frame.
fn route_cache_key(sectors: &[&SectorData], micro_override: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut coords: Vec<(i32, i32)> = sectors
        .iter()
        .filter(|s| !s.routes.is_empty())
        .filter_map(|s| s.info.location.map(|l| (l.x, l.y)))
        .collect();
    coords.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    coords.hash(&mut h);
    micro_override.hash(&mut h);
    h.finish()
}

/// Build the per-color batched geometry for every route in the visible sectors.
fn build_route_buckets(
    sectors: &[&SectorData],
    micro_override: Option<&str>,
) -> Vec<(String, Geometry)> {
    // Group segment endpoints by resolved color, preserving first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut builders: std::collections::HashMap<String, PathBuilder> =
        std::collections::HashMap::new();
    for sector in sectors {
        let Some(loc) = sector.info.location else {
            continue;
        };
        for route in &sector.routes {
            let (Some((sc, sr)), Some((ec, er))) = (parse_hex(&route.start), parse_hex(&route.end))
            else {
                continue;
            };
            let (swc, swr) = world_hex(
                loc.x + route.start_offset.0,
                loc.y + route.start_offset.1,
                sc,
                sr,
            );
            let (ewc, ewr) = world_hex(
                loc.x + route.end_offset.0,
                loc.y + route.end_offset.1,
                ec,
                er,
            );
            // Stop the line short of each world (reference OffsetSegment), in world
            // space so the gap is a constant 0.25 parsec at any zoom.
            let (wp0, wp1) =
                offset_segment(hex_parsec(swc, swr), hex_parsec(ewc, ewr), ROUTE_END_ADJUST);
            // A theme that forces a single micro-route color (Atlas/Print/Draft/FASA/
            // Terminal) wins; else a route's explicit `Color`; else the `otu.css
            // route.<allegiance>` rule, defaulting to "Im" → green.
            let color = match (micro_override, &route.color) {
                (Some(o), _) => o,
                (None, Some(c)) => c.as_str(),
                (None, None) => route_color(route.allegiance.as_deref().unwrap_or("Im")),
            };
            let pb = builders.entry(color.to_string()).or_insert_with(|| {
                order.push(color.to_string());
                PathBuilder::new()
            });
            pb.move_to(wp0.0, wp0.1);
            pb.line_to(wp1.0, wp1.1);
        }
    }
    order
        .into_iter()
        .filter_map(|color| builders.remove(&color).map(|pb| (color, pb.finish())))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_routes(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    dpr: f64,
    sectors: &[&SectorData],
    micro_override: Option<&str>,
    taper: bool,
) {
    let s = view.scale;
    // Reference `routePenWidth`: 0.2 parsec at scale ≤ 16, else 0.08·penScale
    // (penScale = 64/scale past 64); Candy halves it past scale 32.
    let pen_scale = if s <= 64.0 { 1.0 } else { 64.0 / s };
    let mut wparsec = if s <= 16.0 { 0.2 } else { 0.08 * pen_scale };
    if taper && s >= 32.0 {
        wparsec /= 2.0;
    }
    // Stroke width is in world (parsec) units — the transform scales it by `s`,
    // matching the old screen-space `(wparsec·s).max(1)` with a 1-css-px floor.
    let stroke_w = (wparsec * s).max(1.0) / s;
    let style = StrokeStyle::plain(stroke_w);
    // World(parsec) → device, same transform as borders/grid (dpr keeps it crisp).
    let m = Affine::scale_translate(
        dpr * s,
        dpr * (w / 2.0 - view.center.0 * s),
        dpr * (h / 2.0 - view.center.1 * s),
    );
    let key = route_cache_key(sectors, micro_override);
    ROUTE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.as_ref().map(|rc| rc.key) != Some(key) {
            *cache = Some(RouteCache {
                key,
                buckets: build_route_buckets(sectors, micro_override),
            });
        }
        let Some(rc) = cache.as_ref() else { return };
        for (color, geom) in &rc.buckets {
            c.stroke_geometry(geom, m, color, &style, None);
        }
    });
}

/// Draw a computed jump route (from `/api/route`) as a bright highlighted
/// polyline through its waypoints, with a marker dot at each stop. Drawn on top
/// of the map so it stands out from the per-sector trade routes.
pub(crate) fn draw_jump_route(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    route: &RouteResult,
) {
    if route.waypoints.len() < 2 {
        return;
    }
    let pts: Vec<(f64, f64)> = route
        .waypoints
        .iter()
        .map(|wp| view.to_screen(w, h, hex_parsec(wp.coord.x, wp.coord.y)))
        .collect();
    let width = (0.14 * view.scale).max(2.5);
    // Casing (dark, wider) then the bright line, so it reads over any backdrop.
    c.stroke_polyline(&pts, "rgba(0,0,0,0.7)", width + 2.0, false, &[]);
    c.stroke_polyline(&pts, "rgba(80,200,255,0.95)", width, false, &[]);
    let r = (0.18 * view.scale).max(3.0);
    for p in &pts {
        c.fill_circle(p.0, p.1, r, "rgba(80,200,255,0.95)");
    }
}
