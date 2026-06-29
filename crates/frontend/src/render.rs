//! Frontend render entry — adapts the shared, backend-neutral
//! [`tmap_render::render`] scene to a browser `<canvas>`.
//!
//! All the scene logic, LOD thresholds, palette, `Theme`, `ViewState`,
//! `RenderOptions`, and view/transform helpers live in the `tmap-render` crate
//! (re-exported below so existing `render::…` paths keep resolving). The only
//! frontend-specific piece is [`draw`], which acquires the 2D context via
//! [`Canvas2d`] and hands the prepared canvas to [`tmap_render::render::draw_scene`].

pub use tmap_render::render::*;

use std::collections::HashMap;

use tmap_core::dto::{Overlays, RouteResult, SectorData};
use web_sys::HtmlCanvasElement;

use crate::canvas::Canvas2d;

/// Draw the map into a browser `<canvas>`: acquire the 2D context (scaled to the
/// device-pixel-ratio so coordinates are logical CSS pixels), then run the shared
/// scene passes. Mirrors the old `render::draw` signature so `main.rs` is unchanged.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &HtmlCanvasElement,
    sectors: &[&SectorData],
    overlays: Option<&Overlays>,
    sector_index: &HashMap<(i32, i32), String>,
    view: ViewState,
    opts: RenderOptions,
    theme: &Theme,
    route: Option<&RouteResult>,
) {
    let Some((c, w, h, dpr)) = Canvas2d::for_frame(canvas) else {
        return;
    };
    draw_scene(
        &c,
        w,
        h,
        dpr,
        sectors,
        overlays,
        sector_index,
        view,
        opts,
        theme,
        route,
    );
}
