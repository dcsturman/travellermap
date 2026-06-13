//! Jump-N range view — highlights a selected world's jump neighborhood.
//!
//! Mirrors the reference "Jump-N Neighborhood" (`/api/jumpworlds`, every world
//! with `hex_distance ≤ N`), but instead of rendering the separate jumpmap
//! image we light up the in-range worlds directly on the client map: a
//! translucent disc on each world within range, a distinct ring on the origin,
//! and a faint boundary marker on the worlds exactly at the range edge.
//!
//! This is a *range* overlay (an area highlight), not a *route* (the A→B route
//! planner lives in `routes::draw_jump_route`). It reads over the world layer,
//! before the HUD/compass. Distances use the same absolute-hex `Coord`
//! (`world_hex(sx, sy, col, row)`) the route backend builds its waypoints from,
//! so the neighborhood matches the route planner's notion of distance exactly.
//!
//! Only *loaded* worlds are highlighted: the neighborhood may spill into
//! not-yet-streamed sectors, which simply won't light up until they load. No
//! network fetch is triggered here — we highlight over the already-cached
//! `sectors` slice.

use tmap_core::astrometrics::{parse_hex, Coord};
use tmap_core::dto::SectorData;

use crate::canvas::Canvas;

use super::common::{hex_parsec, on_screen, world_hex, RangeView, ViewState};

// Highlight color: the route-highlight cyan from `routes::draw_jump_route`
// (`rgba(80,200,255,…)`) — reused so the two "highlighted on top of the map"
// overlays read as the same UI accent rather than inventing a new value.
const HILITE: &str = "rgba(80,200,255,0.95)";
const HILITE_FILL: &str = "rgba(80,200,255,0.22)"; // low-alpha disc for in-range worlds
const CASING: &str = "rgba(0,0,0,0.7)"; // dark casing under the bright origin ring

/// Highlight every loaded world within `range.jump` parsecs of the origin hex.
pub(crate) fn draw_range(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sectors: &[&SectorData],
    range: RangeView,
) {
    // In-range disc radius and origin-ring stroke scale with zoom so the
    // overlay stays readable at world-detail scale (same idiom as the route
    // highlight's `0.18 * scale` dot / `0.14 * scale` line).
    let disc_r = (0.32 * view.scale).max(4.0);
    let ring_r = (0.40 * view.scale).max(6.0);
    let ring_w = (0.06 * view.scale).max(2.0);

    for sector in sectors {
        let Some(loc) = sector.info.location else {
            continue;
        };
        for wld in &sector.worlds {
            let Some((col, row)) = parse_hex(&wld.hex) else {
                continue;
            };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let dist = range.origin.hex_distance(Coord::new(wc, wr));
            if dist > range.jump {
                continue;
            }
            let (x, y) = view.to_screen(w, h, hex_parsec(wc, wr));
            if !on_screen(x, y, w, h, ring_r) {
                continue;
            }
            if wc == range.origin.x && wr == range.origin.y {
                // Origin world: a distinct bright ring (casing + cyan stroke).
                c.stroke_polyline(&ring_circle(x, y, ring_r), CASING, ring_w + 2.0, true, &[]);
                c.stroke_polyline(&ring_circle(x, y, ring_r), HILITE, ring_w, true, &[]);
            } else {
                // In-range world: translucent cyan disc, brighter at the edge.
                c.fill_circle(x, y, disc_r, HILITE_FILL);
                if dist == range.jump {
                    c.stroke_polyline(&ring_circle(x, y, disc_r), HILITE, ring_w * 0.6, true, &[]);
                }
            }
        }
    }
}

/// A polygon approximating a circle of radius `r` about `(x, y)` — the `Canvas`
/// trait strokes polylines, not arcs, so we tessellate (24 segments reads as a
/// smooth ring at overlay scale).
fn ring_circle(x: f64, y: f64, r: f64) -> Vec<(f64, f64)> {
    (0..24)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / 24.0;
            (x + r * a.cos(), y + r * a.sin())
        })
        .collect()
}
