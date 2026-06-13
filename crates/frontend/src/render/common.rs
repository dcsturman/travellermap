//! Shared rendering infrastructure: view/transform math, layer options, the LOD
//! scale thresholds, the OTU color palette, and the hex/sector geometry helpers
//! that every render pass builds on. No `web-sys` here — pure scene math.

use tmap_core::astrometrics::PARSEC_SCALE_X;

pub(crate) const SECTOR_W: i32 = 32; // parsec columns per sector
pub(crate) const SECTOR_H: i32 = 40; // parsec rows per sector
pub(crate) const SUBSECTOR_W: i32 = 8;
pub(crate) const SUBSECTOR_H: i32 = 10;

/// Zoom clamp (px/parsec). Low floor → full zoom-out to the macro view.
pub const MIN_SCALE: f64 = 0.05;
pub const MAX_SCALE: f64 = 400.0;

/// LOD thresholds (px/parsec), from the reference `Stylesheet`.
pub const WORLD_MIN_SCALE: f64 = 4.0; // worlds appear at/above (dotmap)
pub(crate) const MACRO_MAX_SCALE: f64 = 4.0; // macro overlays below (handoff to micro at 4)
pub(crate) const WORLD_BASIC_SCALE: f64 = 24.0; // starport/name/hex (atlas style)
pub(crate) const WORLD_FULL_SCALE: f64 = 48.0; // poster-style layout
pub(crate) const WORLD_UWP_SCALE: f64 = 96.0; // UWP line above name
pub(crate) const ALLEGIANCE_MIN_SCALE: f64 = 64.0; // allegiance code (T5AllegianceCodeMinScale)
pub(crate) const ROUTE_MIN_SCALE: f64 = 8.0; // routes (RouteMinScale)
pub(crate) const PARSEC_GRID_MIN_SCALE: f64 = 16.0; // per-parsec hex grid (ParsecMinScale)
pub(crate) const STAR_MIN_SCALE: f64 = 3.5; // procedural star field
pub(crate) const MACRO_WORLDS_MIN: f64 = 0.5; // capitals/homeworlds (MacroWorldsMinScale)
pub(crate) const MACRO_WORLDS_MAX: f64 = 4.0; // capitals/homeworlds (MacroWorldsMaxScale)
pub(crate) const SECTOR_GRID_MIN: f64 = 0.5; // sector boundary grid
pub(crate) const SUBSECTOR_GRID_MIN: f64 = 8.0; // subsector boundary grid
pub(crate) const SECTOR_NAME_MIN: f64 = 1.0;
pub(crate) const SECTOR_NAME_MAX: f64 = 16.0;
/// Enlarges in-hex glyph *sizes* (disc, text, icons) so they fill the hex like
/// travellermap.com — the reference's `hexContentScale`. Layout offsets are NOT
/// scaled by this, so larger glyphs stay inside the hex. Tune if text reads too
/// small or large.
pub(crate) const CONTENT_SCALE: f64 = 1.3;
pub(crate) const SUBSECTOR_NAME_MIN: f64 = 24.0;
pub(crate) const SUBSECTOR_NAME_MAX: f64 = 64.0;

// OTU palette (TravellerColors / Stylesheet).
pub(crate) const C_BORDER: &str = "#e32736";
pub(crate) const C_ROUTE: &str = "rgba(235,235,235,0.8)";
pub(crate) const C_RIFT: &str = "rgba(70,72,92,0.55)";
pub(crate) const C_AMBER: &str = "#ffcc00";
pub(crate) const C_RED: &str = "#e32736";
pub(crate) const C_WATER: &str = "#00bfff"; // DeepSkyBlue
pub(crate) const C_DRY: &str = "#ffffff";

/// The reference map renders in Arial (Bold for world names). Matching it
/// keeps text the same width/weight — `system-ui` (San Francisco on macOS) is
/// narrower and reads smaller at the same px size.
pub(crate) const DEFAULT_FONT: &str = "Arial, 'Helvetica Neue', Helvetica, sans-serif";

/// 1/√3 — hex circumradius in parsec units (uniform; the view transform scales it).
pub(crate) const HEX_VR: f64 = 0.577_350_269_189_625_8;

/// What part of the map is on screen: `scale` = pixels per parsec, `center` =
/// the absolute parsec-space point at the canvas center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    pub scale: f64,
    pub center: (f64, f64),
}

/// Layer-visibility / appearance toggles, driven by the hamburger settings menu
/// (mirrors the reference's Features / Appearance switches). Everything on by
/// default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    pub galactic_direction: bool, // COREWARD/RIMWARD/SPINWARD/TRAILING edge labels
    pub sector_grid: bool,        // sector + subsector boundary grids
    pub sector_names: bool,       // sector watermark names
    pub borders: bool,            // polity borders (micro + macro)
    pub routes: bool,             // trade routes (micro + macro)
    pub region_names: bool,       // subsector + region/polity labels
    pub important_worlds: bool,   // capitals + homeworlds (macro dots)
    pub filled_borders: bool,     // tint the interior of bordered regions
    pub more_world_colors: bool,  // color worlds by trade class (vs. plain)
    pub dim_unofficial: bool,     // dim sectors not tagged Official/Preserve/InReview
    pub perf_hud: bool,           // per-layer frame-timing overlay (profiling)
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            galactic_direction: true,
            sector_grid: true,
            sector_names: true,
            borders: true,
            routes: true,
            region_names: true,
            important_worlds: true,
            filled_borders: true,
            more_world_colors: true,
            dim_unofficial: false,
            perf_hud: false,
        }
    }
}

pub(crate) fn world_hex(sx: i32, sy: i32, col: i32, row: i32) -> (i32, i32) {
    (sx * SECTOR_W + col, sy * SECTOR_H + row)
}

/// Parsec-space center of an absolute hex (horizontal compression + even-column
/// stagger baked in; parity continuous across sectors since `SECTOR_W` is even).
pub(crate) fn hex_parsec(wc: i32, wr: i32) -> (f64, f64) {
    let y_off = if wc.rem_euclid(2) == 0 { 0.5 } else { 0.0 };
    (wc as f64 * PARSEC_SCALE_X as f64, wr as f64 + y_off)
}

/// Parsec-space position of an absolute world hex — for the search "jump to".
pub fn world_to_parsec(wc: i32, wr: i32) -> (f64, f64) {
    hex_parsec(wc, wr)
}

/// Parsec center of a world given its sector grid cell `(sx, sy)` and local hex
/// `(col, row)` — for click hit-testing (screen → nearest world) in the UI.
pub fn sector_hex_parsec(sx: i32, sy: i32, col: i32, row: i32) -> (f64, f64) {
    let (wc, wr) = world_hex(sx, sy, col, row);
    hex_parsec(wc, wr)
}

pub fn sector_center(sx: i32, sy: i32) -> (f64, f64) {
    (
        (sx as f64 * SECTOR_W as f64 + 16.5) * PARSEC_SCALE_X as f64,
        sy as f64 * SECTOR_H as f64 + 20.5,
    )
}

impl ViewState {
    pub(crate) fn to_screen(&self, w: f64, h: f64, p: (f64, f64)) -> (f64, f64) {
        (
            w / 2.0 + (p.0 - self.center.0) * self.scale,
            h / 2.0 + (p.1 - self.center.1) * self.scale,
        )
    }

    pub fn to_parsec(&self, w: f64, h: f64, screen: (f64, f64)) -> (f64, f64) {
        (
            self.center.0 + (screen.0 - w / 2.0) / self.scale,
            self.center.1 + (screen.1 - h / 2.0) / self.scale,
        )
    }
}

pub fn fit_sector(w: f64, h: f64, sx: i32, sy: i32) -> ViewState {
    let margin = 24.0;
    let used_w = SECTOR_W as f64 * PARSEC_SCALE_X as f64;
    let used_h = SECTOR_H as f64;
    let scale = ((w - 2.0 * margin) / used_w)
        .min((h - 2.0 * margin) / used_h)
        .clamp(MIN_SCALE, MAX_SCALE);
    ViewState {
        scale,
        center: sector_center(sx, sy),
    }
}

/// The charted-space overview (Home button): a macro-zoom view framing the
/// polity overlays, centered on the Imperium core (parsec origin).
pub fn home_view(w: f64, h: f64) -> ViewState {
    // Approximate absolute-parsec extent of charted space (spinward Zhodani →
    // trailing Hive, coreward Vargr → rimward Solomani).
    let (half_x, half_y) = (135.0, 105.0);
    let margin = 40.0;
    let scale = ((w - 2.0 * margin) / (2.0 * half_x))
        .min((h - 2.0 * margin) / (2.0 * half_y))
        // Stay inside the macro range (< 4) so the polity overlays + capitals/
        // homeworlds are visible at Home regardless of window size.
        .clamp(MIN_SCALE, 3.5);
    ViewState { scale, center: (0.0, 0.0) }
}

pub fn visible_sectors(view: &ViewState, w: f64, h: f64) -> Vec<(i32, i32)> {
    let (wc0, wc1, wr0, wr1) = visible_hex_range(view, w, h);
    let sx0 = (wc0 - 1).div_euclid(SECTOR_W) - 1;
    let sx1 = (wc1 - 1).div_euclid(SECTOR_W) + 1;
    let sy0 = (wr0 - 1).div_euclid(SECTOR_H) - 1;
    let sy1 = (wr1 - 1).div_euclid(SECTOR_H) + 1;
    let mut cells = Vec::new();
    for sx in sx0..=sx1 {
        for sy in sy0..=sy1 {
            cells.push((sx, sy));
        }
    }
    cells
}

/// Inclusive absolute-hex bounds (wc0, wc1, wr0, wr1) covering the viewport (+1 ring).
pub(crate) fn visible_hex_range(view: &ViewState, w: f64, h: f64) -> (i32, i32, i32, i32) {
    let half_w = (w * 0.5) / view.scale;
    let half_h = (h * 0.5) / view.scale;
    (
        ((view.center.0 - half_w) / PARSEC_SCALE_X as f64).floor() as i32 - 1,
        ((view.center.0 + half_w) / PARSEC_SCALE_X as f64).ceil() as i32 + 1,
        (view.center.1 - half_h).floor() as i32 - 1,
        (view.center.1 + half_h).ceil() as i32 + 1,
    )
}

/// A hex vertex (k = 0..5, flat-top, k0 to the right) at circumradius `r`, in
/// parsec space.
pub(crate) fn hex_vertex_r(wc: i32, wr: i32, k: usize, r: f64) -> (f64, f64) {
    let (cx, cy) = hex_parsec(wc, wr);
    let a = std::f64::consts::FRAC_PI_3 * k as f64;
    (cx + r * a.cos(), cy + r * a.sin())
}

/// A hex vertex at the exact tiling radius (for boundary edges).
pub(crate) fn hex_vertex(wc: i32, wr: i32, k: usize) -> (f64, f64) {
    hex_vertex_r(wc, wr, k, HEX_VR)
}

/// The 6 neighbors of an absolute hex, each with the local vertex pair (edge)
/// facing it. Even columns are staggered +0.5 row (matches `hex_parsec`).
pub(crate) fn hex_neighbors(wc: i32, wr: i32) -> [((i32, i32), (usize, usize)); 6] {
    if wc.rem_euclid(2) == 0 {
        [
            ((wc, wr - 1), (4, 5)),     // up
            ((wc, wr + 1), (1, 2)),     // down
            ((wc + 1, wr), (5, 0)),     // upper-right
            ((wc + 1, wr + 1), (0, 1)), // lower-right
            ((wc - 1, wr), (3, 4)),     // upper-left
            ((wc - 1, wr + 1), (2, 3)), // lower-left
        ]
    } else {
        [
            ((wc, wr - 1), (4, 5)),
            ((wc, wr + 1), (1, 2)),
            ((wc + 1, wr - 1), (5, 0)),
            ((wc + 1, wr), (0, 1)),
            ((wc - 1, wr - 1), (3, 4)),
            ((wc - 1, wr), (2, 3)),
        ]
    }
}

/// The sector grid cell a world hex belongs to.
pub(crate) fn hex_sector(wc: i32, wr: i32) -> (i32, i32) {
    ((wc - 1).div_euclid(SECTOR_W), (wr - 1).div_euclid(SECTOR_H))
}

pub(crate) fn on_screen(x: f64, y: f64, w: f64, h: f64, pad: f64) -> bool {
    x >= -pad && x <= w + pad && y >= -pad && y <= h + pad
}

/// Does this sector's bounding box overlap the viewport (+~1 hex margin)? Used
/// to skip off-screen sectors in the `sectors` slice so we don't stroke their
/// whole grids.
pub(crate) fn sector_in_viewport(loc: (i32, i32), view: &ViewState, w: f64, h: f64) -> bool {
    let (c0, c1) = (loc.0 * SECTOR_W + 1, loc.0 * SECTOR_W + SECTOR_W);
    let (r0, r1) = (loc.1 * SECTOR_H + 1, loc.1 * SECTOR_H + SECTOR_H);
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &c in &[c0, c1] {
        for &r in &[r0, r1] {
            let (x, y) = view.to_screen(w, h, hex_parsec(c, r));
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    let m = view.scale;
    maxx >= -m && minx <= w + m && maxy >= -m && miny <= h + m
}

/// Border stroke color by allegiance — ported from `res/styles/otu.css`
/// (specific 4-char codes first, then the 2-char prefix fallback, then gray).
pub(crate) fn allegiance_border_color(a: &str) -> &'static str {
    match a {
        "ImDa" | "ImDc" | "ImDd" | "ImDg" | "ImDi" | "ImDs" | "ImDv" => "#E32736",
        "ImLa" | "ImSy" | "ImLc" | "ImAp" | "ImLu" | "ImVd" => "#0000ff",
        "SoCf" => "#ffa500",
        "SoNS" | "SoRD" | "SoWu" => "#0000ff",
        "ZhCa" | "ZhCh" | "ZhCo" | "ZhIa" | "ZhIN" | "ZhJp" | "ZhMe" | "ZhOb" | "ZhSh" | "ZhVQ" => "#0000ff",
        "AsXX" => "#ffff00",
        "HvFd" => "#800080",
        "KkTw" => "#008000",
        "JuPr" => "#0000ff",
        "JAOz" => "#008080",
        "JAsi" => "#add8e6",
        "JCoK" => "#00ffff",
        "JHhk" => "#add8e6",
        "JLum" => "#00ffff",
        "JMen" => "#008080",
        "JPSt" => "#7fffd4",
        "JRar" => "#6b8e23",
        "JUkh" => "#008080",
        "JVug" => "#add8e6",
        "JuHl" => "#4682b4",
        "JuRu" => "#00ffff",
        _ => match a.get(..2).unwrap_or(a) {
            "Im" => "#E32736",
            "Zh" => "#0000ff",
            "Kk" => "#008000",
            "As" => "#ffff00",
            _ => "#808080", // microBorders default (gray)
        },
    }
}
