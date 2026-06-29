//! `GET /api/tile` — a poster-style map tile, rendered server-side as **SVG**.
//!
//! The reference (`TileHandler.cs`) renders a PNG via `System.Drawing`; this
//! rewrite moved rendering to the browser, so the backend normally emits no
//! images. This one endpoint is the exception: external `<img>` consumers (e.g.
//! worldgen's "Route Map") need a self-contained image URL, and an `<img>` can't
//! run our client-side WASM renderer. So we drive the *same* shared render passes
//! ([`tmap_render::render::draw_scene`]) through [`SvgCanvas`] and return vector
//! SVG — using the viewer's fonts, hence a local-deployment convenience rather
//! than pixel-faithful server rasterization.
//!
//! Tile geometry matches the reference exactly (`TileHandler.cs` + `Astrometrics`):
//! `x,y` are in tile-width units, and the centered view in the frontend's
//! compressed world units works out to `center = ((x+0.5)·w/scale,
//! (y+0.5)·h/scale)` (the `PARSEC_SCALE_X` factor cancels).

use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use tmap_core::astrometrics::{PARSEC_SCALE_X, REFERENCE_HEX_X, REFERENCE_HEX_Y};
use tmap_core::dto::SectorData;
use tmap_render::render::{self, visible_sectors, RenderOptions, Theme, ViewState};

use crate::svg_canvas::SvgCanvas;
use crate::{build_overlays, build_sector_data, AppState};

/// Max tile edge in pixels (reference `TileHandler.MaxDimension`).
const MAX_DIM: u32 = 2048;
/// Scale clamp in px/parsec (reference `ImageHandlerBase` `2^-7 … 2^9`).
const MIN_SCALE: f64 = 0.007_812_5;
const MAX_SCALE: f64 = 512.0;
/// Reference `TileHandler` default options: SectorGrid | BordersMajor |
/// NamesMajor | NamesMinor.
const DEFAULT_OPTIONS: i64 = 0x0001 | 0x0010 | 0x0040 | 0x0080;

#[derive(Deserialize)]
pub struct TileQuery {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default = "default_dim")]
    w: u32,
    #[serde(default = "default_dim")]
    h: u32,
    #[serde(default)]
    scale: f64,
    #[serde(default)]
    options: Option<i64>,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    milieu: Option<String>,
}

fn default_dim() -> u32 {
    256
}

/// Map the reference `MapOptions` bitfield onto our `RenderOptions`. (Routes are
/// scale-gated, not in the bitfield, so they're always enabled.)
fn options_from_bits(bits: i64) -> RenderOptions {
    let b = bits as u32;
    const SECTOR_GRIDS: u32 = 0x0003; // SectorGrid | SubsectorGrid
    const SECTORS_MASK: u32 = 0x000C; // SectorsSelected | SectorsAll
    const BORDERS_MASK: u32 = 0x0030; // BordersMajor | BordersMinor
    const NAMES_MASK: u32 = 0x00C0; // NamesMajor | NamesMinor
    const WORLDS_MASK: u32 = 0x0300; // WorldsCapitals | WorldsHomeworlds
    const WORLD_COLORS: u32 = 0x4000;
    const FILLED_BORDERS: u32 = 0x8000;
    RenderOptions {
        sector_grid: b & SECTOR_GRIDS != 0,
        sector_names: b & SECTORS_MASK != 0,
        borders: b & BORDERS_MASK != 0,
        region_names: b & NAMES_MASK != 0,
        important_worlds: b & WORLDS_MASK != 0,
        filled_borders: b & FILLED_BORDERS != 0,
        more_world_colors: b & WORLD_COLORS != 0,
        routes: true,
        // The compass overlay isn't part of a tile (the reference doesn't draw it).
        galactic_direction: false,
        ..RenderOptions::default()
    }
}

pub async fn get_tile(Query(q): Query<TileQuery>, State(state): State<AppState>) -> Response {
    let w = q.w.clamp(1, MAX_DIM);
    let h = q.h.clamp(1, MAX_DIM);
    let scale = q.scale.clamp(MIN_SCALE, MAX_SCALE);
    let milieu = q.milieu.unwrap_or_else(|| "M1105".to_string());
    let style = q.style.unwrap_or_else(|| "poster".to_string());
    let bits = q.options.unwrap_or(DEFAULT_OPTIONS);
    let (x, y) = (q.x, q.y);

    // Rendering reads files + builds geometry (CPU); keep it off the async
    // reactor. Rarely called, so a plain blocking render per request is fine.
    let render = tokio::task::spawn_blocking(move || {
        render_tile(&state, &milieu, x, y, w, h, scale, bits, &style)
    })
    .await;

    match render {
        Ok(Ok(svg)) => (
            [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
            svg,
        )
            .into_response(),
        Ok(Err((code, msg))) => (code, msg).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "tile render task failed".to_string(),
        )
            .into_response(),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tile(
    state: &AppState,
    milieu: &str,
    x: f64,
    y: f64,
    w: u32,
    h: u32,
    scale: f64,
    bits: i64,
    style: &str,
) -> Result<String, (StatusCode, String)> {
    let (wf, hf) = (f64::from(w), f64::from(h));
    // The tile rect's center in the *reference* world-coordinate system (the one
    // worldgen's x,y address, exactly as TileHandler.cs computes it): x,y are
    // tile-width units, so the center is `(x+0.5)·w / (scale·PSX)` etc.
    let psx = f64::from(PARSEC_SCALE_X);
    let cx_ref = (x + 0.5) * wf / (scale * psx);
    let cy_ref = (y + 0.5) * hf / scale;
    // Reference coords → render world space. Two adjustments:
    //  1. Frame origin: the render passes index hexes as `wc = sx·32 + hx`,
    //     `wr = sy·40 + hy`, whereas the reference subtracts REFERENCE_HEX_X/Y
    //     (1, 40) — so the render frame is shifted by (+1, +40) hex units, and x
    //     is then horizontally compressed by PSX (`hex_parsec`).
    //  2. Hex-center convention: the reference's `HexToCenter` places a hex at
    //     `(X − 0.5, Y − [X even ? 0.5 : 0])`, while `hex_parsec` uses
    //     `(wc, wr + [wc even ? 0.5 : 0])`. Net, our world positions sit a uniform
    //     (+0.5, +0.5) parsec off the reference — invisible on the standalone map,
    //     but it would slide an external overlay (worldgen's route) off our worlds.
    //     Adding 0.5 to the view center cancels it so tiles match the reference
    //     pixel-for-pixel.
    let view = ViewState {
        scale,
        center: (
            (cx_ref + f64::from(REFERENCE_HEX_X) + 0.5) * psx,
            cy_ref + f64::from(REFERENCE_HEX_Y) + 0.5,
        ),
    };

    let universe = state.universe(milieu)?;
    // Grid cell → sector name, for both world lookup and the sector-name labels.
    let sector_index: HashMap<(i32, i32), String> = universe
        .sectors
        .iter()
        .map(|e| ((e.location.x, e.location.y), e.name.clone()))
        .collect();

    // Build the sectors whose grid cells fall in the viewport.
    let data: Vec<SectorData> = visible_sectors(&view, wf, hf)
        .into_iter()
        .filter_map(|cell| sector_index.get(&cell))
        .filter_map(|name| build_sector_data(state, milieu, name, "full").ok())
        .collect();
    let refs: Vec<&SectorData> = data.iter().collect();

    let overlays = state
        .overlays
        .get_or_init(|| build_overlays(&state.res_dir));

    let opts = options_from_bits(bits);
    let theme = Theme::from_name(style);

    let svg = SvgCanvas::new();
    render::draw_scene(
        &svg,
        wf,
        hf,
        1.0,
        &refs,
        Some(overlays),
        &sector_index,
        view,
        opts,
        &theme,
        None,
    );
    Ok(svg.into_svg(wf, hf))
}
