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

use crate::canvas::{Canvas, Canvas2d};

// ── Search "found it" pulse ──────────────────────────────────────────────────
// A brief blue attention-getter drawn on top of the rendered scene after a
// search jumps the view to a world: three rings born in sequence, each
// expanding and fading, so the eye is drawn to the target amid a dense field.

/// Number of rings in one pulse.
pub const PULSE_RINGS: u32 = 3;
/// Milliseconds between successive ring births.
const PULSE_STAGGER_MS: f64 = 260.0;
/// Lifetime of a single ring (birth → fully faded).
const PULSE_LIFE_MS: f64 = 1000.0;
/// Total animation length — the driver (rAF loop) runs for exactly this long.
pub const PULSE_DURATION_MS: f64 = PULSE_STAGGER_MS * (PULSE_RINGS as f64 - 1.0) + PULSE_LIFE_MS;

/// Draw the search pulse on top of the already-rendered scene: up to
/// [`PULSE_RINGS`] blue rings radiating out from `center_parsec` (absolute
/// parsec coords), staggered and fading. `elapsed_ms` is the time since the
/// pulse began. The caller repaints the scene every frame (a rAF loop) and calls
/// this afterward. Returns `true` while the animation is still running.
pub fn draw_pulse(
    canvas: &HtmlCanvasElement,
    view: ViewState,
    center_parsec: (f64, f64),
    elapsed_ms: f64,
) -> bool {
    const R0: f64 = 7.0; // birth radius (logical px)
    const R_MAX: f64 = 60.0; // death radius (logical px)

    let Some((c, w, h, _dpr)) = Canvas2d::for_frame(canvas) else {
        return false;
    };
    // Inline the (crate-private) view transform: parsec → logical screen px.
    let sx = w / 2.0 + (center_parsec.0 - view.center.0) * view.scale;
    let sy = h / 2.0 + (center_parsec.1 - view.center.1) * view.scale;

    for i in 0..PULSE_RINGS {
        let age = elapsed_ms - i as f64 * PULSE_STAGGER_MS;
        if !(0.0..=PULSE_LIFE_MS).contains(&age) {
            continue;
        }
        let t = age / PULSE_LIFE_MS; // 0 → 1 over the ring's life
        let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out: fast then settle
        let r = R0 + (R_MAX - R0) * ease;
        let alpha = (1.0 - t) * 0.85; // fade as it expands
        let width = 1.0 + 2.5 * (1.0 - t); // thin out as it grows
        let color = format!("rgba(90,180,255,{alpha:.3})");
        c.stroke_arc(sx, sy, r, 0.0, std::f64::consts::TAU, &color, width);
    }
    elapsed_ms < PULSE_DURATION_MS
}

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
