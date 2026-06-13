//! Micro (per-sector) trade routes — green polylines between hex endpoints.

use tmap_core::astrometrics::parse_hex;
use tmap_core::dto::{RouteResult, SectorData};

use crate::canvas::Canvas;

use super::common::{hex_parsec, world_hex, ViewState};

pub(crate) fn draw_routes(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
    let Some(loc) = sector.info.location else {
        return;
    };
    let width = (0.08 * view.scale).max(1.5);
    for route in &sector.routes {
        let (Some((sc, sr)), Some((ec, er))) = (parse_hex(&route.start), parse_hex(&route.end)) else {
            continue;
        };
        let (swc, swr) = world_hex(loc.x + route.start_offset.0, loc.y + route.start_offset.1, sc, sr);
        let (ewc, ewr) = world_hex(loc.x + route.end_offset.0, loc.y + route.end_offset.1, ec, er);
        let p0 = view.to_screen(w, h, hex_parsec(swc, swr));
        let p1 = view.to_screen(w, h, hex_parsec(ewc, ewr));
        c.stroke_polyline(&[p0, p1], "rgba(60,170,70,0.85)", width, false, &[]);
    }
}

/// Draw a computed jump route (from `/api/route`) as a bright highlighted
/// polyline through its waypoints, with a marker dot at each stop. Drawn on top
/// of the map so it stands out from the per-sector trade routes.
pub(crate) fn draw_jump_route(c: &impl Canvas, view: &ViewState, w: f64, h: f64, route: &RouteResult) {
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
