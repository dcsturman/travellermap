//! Micro (per-sector) trade routes — green polylines between hex endpoints.

use tmap_core::astrometrics::parse_hex;
use tmap_core::dto::SectorData;

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
