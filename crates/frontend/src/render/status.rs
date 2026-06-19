//! Map "status" overlay: a translucent dim wash over sectors that aren't
//! official/reviewed (mirrors the reference site's treatment of unofficial
//! sectors). A screen-space pass drawn after the world layers.
//!
//! (The per-sector data-source credit is rendered as an HTML footer in
//! `main.rs`, not on the canvas.)

use super::common::{hex_parsec, on_screen, ViewState, SECTOR_H, SECTOR_W};
use crate::canvas::Canvas;
use tmap_core::dto::SectorData;

/// Tags that mark a sector as "official enough" to draw at full brightness.
const OFFICIAL_TAGS: [&str; 3] = ["Official", "Preserve", "InReview"];

/// Dim every sector that isn't tagged official/reviewed (empty tags count as
/// unofficial) with a translucent dark quad over its bounding box.
#[allow(dead_code)]
pub(crate) fn draw_dim_overlay(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sectors: &[&SectorData],
) {
    for sd in sectors {
        let Some(loc) = sd.info.location else {
            continue;
        };
        if OFFICIAL_TAGS.iter().any(|t| sd.info.tags.contains(t)) {
            continue; // official / reviewed → full brightness
        }
        // Four corner world hexes of the sector (cols 1..=W, rows 1..=H).
        let (c0, c1) = (loc.x * SECTOR_W + 1, loc.x * SECTOR_W + SECTOR_W);
        let (r0, r1) = (loc.y * SECTOR_H + 1, loc.y * SECTOR_H + SECTOR_H);
        let corners = [(c0, r0), (c1, r0), (c1, r1), (c0, r1)];
        let quad: Vec<(f64, f64)> = corners
            .iter()
            .map(|&(wc, wr)| view.to_screen(w, h, hex_parsec(wc, wr)))
            .collect();
        // Skip if the whole quad is off-screen.
        if quad.iter().all(|&(x, y)| !on_screen(x, y, w, h, 0.0)) {
            continue;
        }
        c.fill_polygons(&[quad], "#000000", 0.45);
    }
}
