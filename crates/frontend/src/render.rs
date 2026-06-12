//! Map rendering — scene logic only, expressed against `trait Canvas`.
//!
//! Phase 6: OTU styling (`Stylesheet` palette) + LOD detail tiers. Macro view
//! gets red polity borders, white dashed routes, region labels and a star
//! field; the close view colors worlds (water/dry, amber/red zones) and reveals
//! names then UWPs as you zoom in. Absolute parsec coordinates throughout. This
//! module knows nothing about `web-sys`.

use std::cell::RefCell;
use std::collections::HashMap;

use tmap_core::astrometrics::{parse_hex, PARSEC_SCALE_X};
use tmap_core::dto::{Overlays, SectorData, VectorObject, World};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, Path2d};

use crate::canvas::{Canvas, Canvas2d, TextAlign};
use crate::glyph;

const SECTOR_W: i32 = 32; // parsec columns per sector
const SECTOR_H: i32 = 40; // parsec rows per sector
const SUBSECTOR_W: i32 = 8;
const SUBSECTOR_H: i32 = 10;

/// Zoom clamp (px/parsec). Low floor → full zoom-out to the macro view.
pub const MIN_SCALE: f64 = 0.05;
pub const MAX_SCALE: f64 = 400.0;

/// LOD thresholds (px/parsec), from the reference `Stylesheet`.
pub const WORLD_MIN_SCALE: f64 = 4.0; // worlds appear at/above (dotmap)
pub const MACRO_MAX_SCALE: f64 = 4.0; // macro overlays below (handoff to micro at 4)
const WORLD_BASIC_SCALE: f64 = 24.0; // starport/name/hex (atlas style)
const WORLD_FULL_SCALE: f64 = 48.0; // poster-style layout
const WORLD_UWP_SCALE: f64 = 96.0; // UWP line above name
const ALLEGIANCE_MIN_SCALE: f64 = 64.0; // allegiance code (T5AllegianceCodeMinScale)
const ROUTE_MIN_SCALE: f64 = 8.0; // routes (RouteMinScale)
const PARSEC_GRID_MIN_SCALE: f64 = 16.0; // per-parsec hex grid (ParsecMinScale)
const STAR_MIN_SCALE: f64 = 3.5; // procedural star field
const MACRO_WORLDS_MIN: f64 = 0.5; // capitals/homeworlds (MacroWorldsMinScale)
const MACRO_WORLDS_MAX: f64 = 4.0; // capitals/homeworlds (MacroWorldsMaxScale)
const SECTOR_GRID_MIN: f64 = 0.5; // sector boundary grid
const SUBSECTOR_GRID_MIN: f64 = 8.0; // subsector boundary grid
const SECTOR_NAME_MIN: f64 = 1.0;
const SECTOR_NAME_MAX: f64 = 16.0;
/// Enlarges in-hex glyph *sizes* (disc, text, icons) so they fill the hex like
/// travellermap.com — the reference's `hexContentScale`. Layout offsets are NOT
/// scaled by this, so larger glyphs stay inside the hex. Tune if text reads too
/// small or large.
const CONTENT_SCALE: f64 = 1.3;
const SUBSECTOR_NAME_MIN: f64 = 24.0;
const SUBSECTOR_NAME_MAX: f64 = 64.0;

// OTU palette (TravellerColors / Stylesheet).
const C_BORDER: &str = "#e32736";
const C_ROUTE: &str = "rgba(235,235,235,0.8)";
const C_RIFT: &str = "rgba(70,72,92,0.55)";
const C_AMBER: &str = "#ffcc00";
const C_RED: &str = "#e32736";
const C_WATER: &str = "#00bfff"; // DeepSkyBlue
const C_DRY: &str = "#ffffff";

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
            perf_hud: false,
        }
    }
}

fn world_hex(sx: i32, sy: i32, col: i32, row: i32) -> (i32, i32) {
    (sx * SECTOR_W + col, sy * SECTOR_H + row)
}

/// Parsec-space center of an absolute hex (horizontal compression + even-column
/// stagger baked in; parity continuous across sectors since `SECTOR_W` is even).
fn hex_parsec(wc: i32, wr: i32) -> (f64, f64) {
    let y_off = if wc.rem_euclid(2) == 0 { 0.5 } else { 0.0 };
    (wc as f64 * PARSEC_SCALE_X as f64, wr as f64 + y_off)
}

/// Parsec-space position of an absolute world hex — for the search "jump to".
pub fn world_to_parsec(wc: i32, wr: i32) -> (f64, f64) {
    hex_parsec(wc, wr)
}

pub fn sector_center(sx: i32, sy: i32) -> (f64, f64) {
    (
        (sx as f64 * SECTOR_W as f64 + 16.5) * PARSEC_SCALE_X as f64,
        sy as f64 * SECTOR_H as f64 + 20.5,
    )
}

impl ViewState {
    fn to_screen(&self, w: f64, h: f64, p: (f64, f64)) -> (f64, f64) {
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
fn visible_hex_range(view: &ViewState, w: f64, h: f64) -> (i32, i32, i32, i32) {
    let half_w = (w * 0.5) / view.scale;
    let half_h = (h * 0.5) / view.scale;
    (
        ((view.center.0 - half_w) / PARSEC_SCALE_X as f64).floor() as i32 - 1,
        ((view.center.0 + half_w) / PARSEC_SCALE_X as f64).ceil() as i32 + 1,
        (view.center.1 - half_h).floor() as i32 - 1,
        (view.center.1 + half_h).ceil() as i32 + 1,
    )
}

/// Draw the map under the current view, choosing layers by LOD.
pub fn draw(
    canvas: &HtmlCanvasElement,
    sectors: &[&SectorData],
    overlays: Option<&Overlays>,
    sector_index: &HashMap<(i32, i32), String>,
    view: ViewState,
    opts: RenderOptions,
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
    draw_stars(&c, &view, w, h);
    mark("stars", &mut marks);

    if view.scale < MACRO_MAX_SCALE {
        if let Some(ov) = overlays {
            draw_overlays(&c, &view, w, h, ov, opts);
        }
    }
    // Capitals + homeworlds (Worlds.xml) over the macro view (scale 0.5–4).
    if opts.important_worlds && (MACRO_WORLDS_MIN..=MACRO_WORLDS_MAX).contains(&view.scale) {
        if let Some(ov) = overlays {
            draw_world_labels(&c, &view, w, h, ov);
        }
    }
    mark("macro", &mut marks);

    // Sector / subsector boundary grids and background names.
    if opts.sector_grid && view.scale >= SUBSECTOR_GRID_MIN {
        draw_grid_lines(&c, &view, w, h, SUBSECTOR_W, SUBSECTOR_H, "rgba(140,160,200,0.34)", 1.2);
    }
    if opts.sector_grid && view.scale >= SECTOR_GRID_MIN {
        draw_grid_lines(&c, &view, w, h, SECTOR_W, SECTOR_H, "rgba(170,190,225,0.55)", 1.4);
    }
    if opts.sector_names && (SECTOR_NAME_MIN..=SECTOR_NAME_MAX).contains(&view.scale) {
        draw_sector_names(&c, &view, w, h, sector_index);
    }
    if opts.region_names && (SUBSECTOR_NAME_MIN..=SUBSECTOR_NAME_MAX).contains(&view.scale) {
        for sector in sectors {
            draw_subsector_names(&c, &view, w, h, sector);
        }
    }
    mark("grid+names", &mut marks);

    if view.scale >= WORLD_MIN_SCALE {
        // Micro borders (fill behind everything, then stroke).
        if opts.borders {
            draw_micro_borders(&c, &view, w, h, dpr, sectors, opts.filled_borders);
        }
        mark("borders", &mut marks);
        if opts.routes && view.scale >= ROUTE_MIN_SCALE {
            for sector in sectors {
                draw_routes(&c, &view, w, h, sector);
            }
        }
        // Per-parsec hex grid only once hexes are big enough to read (and to
        // avoid drawing tens of thousands of hexagons when zoomed out).
        if opts.sector_grid && view.scale >= PARSEC_GRID_MIN_SCALE {
            draw_hex_grid(&c, &view, w, h, dpr, sector_index);
        }
        mark("routes+hexgrid", &mut marks);
        // Disc / zone-ring / vacuum-outline layer: identical geometry at every
        // detail scale (the reference's in-hex disc), so always drawn from the
        // batched, cached per-sector dot paths — a few fills, not a call/world.
        draw_world_dots(&c, &view, w, h, dpr, sectors, opts.more_world_colors);
        // At basic scale and up, add the per-world text + small glyphs (hex#,
        // starport, gas giant, bases, UWP, allegiance, name) in state-batched
        // passes: canvas font/fill/align set once per pass, not once per glyph.
        if view.scale >= WORLD_BASIC_SCALE {
            draw_world_glyphs(&c, &view, w, h, sectors);
        }
        mark("worlds", &mut marks);
        // Border labels ("Third Imperium") once names are legible.
        if opts.region_names && view.scale >= 16.0 {
            for sector in sectors {
                draw_border_labels(&c, &view, w, h, sector);
            }
        }
    }

    // Compass labels last, on top of everything, at every zoom.
    if opts.galactic_direction {
        draw_galactic_directions(&c, w, h);
    }
    mark("labels+misc", &mut marks);

    if opts.perf_hud {
        draw_perf_hud(&c, w, h, &marks, sectors.len(), view.scale);
    }
}

/// `performance.now()` in milliseconds (monotonic, sub-ms). Returns 0 if
/// unavailable (e.g. no window) so timing degrades gracefully.
fn now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}

/// Profiling overlay: per-layer milliseconds + total/fps, bottom-left. Layers
/// over 4 ms are flagged red. Toggle via the settings menu ("Frame Timing").
fn draw_perf_hud(c: &impl Canvas, _w: f64, h: f64, marks: &[(&str, f64)], n_sectors: usize, scale: f64) {
    let total: f64 = marks.iter().map(|(_, ms)| ms).sum();
    let mono = "12px ui-monospace, Menlo, monospace";
    let line_h = 15.0;
    let x = 12.0;
    let box_h = (marks.len() as f64 + 3.0) * line_h + 14.0;
    let top = h - box_h - 30.0; // sit above the footer
    c.fill_polygons(
        &[vec![(x - 6.0, top - 6.0), (x + 204.0, top - 6.0), (x + 204.0, top + box_h - 6.0), (x - 6.0, top + box_h - 6.0)]],
        "#0b0e16",
        0.84,
    );
    let mut y = top + line_h;
    let fps = if total > 0.0 { 1000.0 / total } else { 0.0 };
    c.fill_text(&format!("FRAME  {total:5.1} ms   {fps:3.0} fps"), x, y, "#9ef0a0", mono, TextAlign::Left);
    y += line_h;
    c.fill_text(&format!("scale {scale:6.1}   sectors {n_sectors}"), x, y, "#aab3c8", mono, TextAlign::Left);
    y += line_h;
    for (label, ms) in marks {
        let col = if *ms > 4.0 { "#ffb0b0" } else { "#c9d2e4" };
        c.fill_text(&format!("{label:<14}{ms:6.1}"), x, y, col, mono, TextAlign::Left);
        y += line_h;
    }
    // Border cache detail: is this frame a rebuild (expensive) or cached redraw?
    let bs = BORDER_STATS.with(|s| s.get());
    let (bline, bcol) = if bs.rebuilt {
        (format!("↻ BUILD {}grp {}hex {:.1}ms", bs.groups, bs.hexes, bs.build_ms), "#ffd0a0")
    } else {
        (format!("border cached {}grp", bs.groups), "#9aa3b8")
    };
    c.fill_text(&bline, x, y, bcol, mono, TextAlign::Left);
}

/// Screen-fixed COREWARD / RIMWARD / SPINWARD / TRAILING compass labels at the
/// viewport edges (the reference's galactic-direction overlay). Red, like the
/// reference; spinward/trailing read vertically.
fn draw_galactic_directions(c: &impl Canvas, w: f64, h: f64) {
    const COLOR: &str = "rgba(227,39,54,0.78)";
    let font = format!("700 15px {DEFAULT_FONT}");
    let cx = w / 2.0;
    let cy = h / 2.0;
    use std::f64::consts::FRAC_PI_2;
    c.fill_text("COREWARD", cx, 20.0, COLOR, &font, TextAlign::Center);
    c.fill_text("RIMWARD", cx, h - 34.0, COLOR, &font, TextAlign::Center);
    c.fill_text_rotated("SPINWARD", 18.0, cy, COLOR, &font, -FRAC_PI_2, 1.0);
    c.fill_text_rotated("TRAILING", w - 18.0, cy, COLOR, &font, FRAC_PI_2, 1.0);
}

/// 1/√3 — hex circumradius in parsec units (uniform; the view transform scales it).
const HEX_VR: f64 = 0.577_350_269_189_625_8;

/// A hex vertex (k = 0..5, flat-top, k0 to the right) at circumradius `r`, in
/// parsec space.
fn hex_vertex_r(wc: i32, wr: i32, k: usize, r: f64) -> (f64, f64) {
    let (cx, cy) = hex_parsec(wc, wr);
    let a = std::f64::consts::FRAC_PI_3 * k as f64;
    (cx + r * a.cos(), cy + r * a.sin())
}

/// A hex vertex at the exact tiling radius (for boundary edges).
fn hex_vertex(wc: i32, wr: i32, k: usize) -> (f64, f64) {
    hex_vertex_r(wc, wr, k, HEX_VR)
}

/// The 6 neighbors of an absolute hex, each with the local vertex pair (edge)
/// facing it. Even columns are staggered +0.5 row (matches `hex_parsec`).
fn hex_neighbors(wc: i32, wr: i32) -> [((i32, i32), (usize, usize)); 6] {
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

/// Allegiance borders: filled interior + exact hex-edge outline. Region hexes
/// are grouped **by allegiance across all loaded sectors**, so a polity that
/// spans sector boundaries (the Imperium!) is one continuous region — no false
/// borders or doubled fill at the seams. Each group fills its hexagons (union)
/// and strokes only edges where the region meets a non-region hex.
/// Grouping key for borders. The major polities span many sectors under
/// different sub-codes (Imperium domains `ImDd`/`ImDv`/…, Aslan clans
/// `AsXX`/`AsVc`/…) but are one continuous region — merge them by their 2-char
/// prefix so domain/clan boundaries inside them aren't stroked as edges. Pocket
/// empires keep their full code so distinct neighbors still get a border.
fn border_group_key(allegiance: &str) -> &str {
    match allegiance.get(..2) {
        Some(p @ ("Im" | "As" | "Zh" | "So" | "Va" | "Hv" | "Kk")) => p,
        _ => allegiance,
    }
}

/// Cached per-group border geometry in WORLD (parsec) coordinates: a fill path
/// (inflated hexagons) + a stroke path (region↔non-region edges). Built once per
/// visible-sector set and re-drawn each frame under a canvas transform, so a pan
/// doesn't re-issue tens of thousands of wasm→JS path calls per frame (this was
/// the dominant zoomed-out cost the frame-timing HUD surfaced).
struct BorderCache {
    key: u64,
    groups: Vec<(String, Path2d, Path2d)>,
}
thread_local! {
    static BORDER_CACHE: RefCell<Option<BorderCache>> = const { RefCell::new(None) };
}

/// Last-frame border render stats, surfaced in the perf HUD so we can see
/// whether a spike is a cache *rebuild* (expensive) or just the cached redraw.
#[derive(Default, Clone, Copy)]
struct BorderStats {
    rebuilt: bool,
    build_ms: f64,
    groups: usize,
    hexes: usize,
}
thread_local! {
    static BORDER_STATS: std::cell::Cell<BorderStats> =
        const { std::cell::Cell::new(BorderStats { rebuilt: false, build_ms: 0.0, groups: 0, hexes: 0 }) };
}

/// Hash of the on-screen *bordered* sectors (+ fill flag). The cache rebuilds
/// only when this set changes (sectors stream in / zoom changes the set), not on
/// every pan frame — between changes the cached `Path2d`s are just re-transformed.
fn border_cache_key(sectors: &[&SectorData], filled: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut coords: Vec<(i32, i32)> = sectors
        .iter()
        .filter(|s| s.borders.iter().any(|b| !b.region.is_empty()))
        .filter_map(|s| s.info.location.map(|l| (l.x, l.y)))
        .collect();
    coords.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    coords.hash(&mut h);
    filled.hash(&mut h);
    h.finish()
}

/// Persistent **per-sector** border geometry: each border group's fill `Path2d`
/// (inflated hexagons, world coords) + its region hex list. Built once per
/// sector (≤ the ~190 sectors in a milieu) so a per-frame combine never
/// re-emits the tens of thousands of fill-vertex calls — it just `add_path`s
/// the cached fills and rebuilds the (cheap, cross-sector) boundary stroke.
struct SectorGeom {
    groups: Vec<SectorGroup>,
}
struct SectorGroup {
    key: String,
    color: String,
    fill: Path2d,
    /// Boundary edges to **same-sector** non-region neighbors — determined by
    /// this sector's region alone, so cached once (the bulk of the stroke).
    interior_stroke: Path2d,
    /// This sector's region as a set, for neighbor seam lookups.
    rset: std::collections::HashSet<(i32, i32)>,
    /// Candidate edges to neighbors in an **adjacent sector**, with vertices
    /// precomputed. Each is stroked at combine only if that neighbor isn't in
    /// the same group's region in its sector (resolved without a merged set).
    seams: Vec<SeamEdge>,
}
/// A precomputed cross-sector border-edge candidate (world-space vertices).
struct SeamEdge {
    nb_cell: (i32, i32),
    nb: (i32, i32),
    a: (f64, f64),
    b: (f64, f64),
}

/// The sector grid cell a world hex belongs to.
fn hex_sector(wc: i32, wr: i32) -> (i32, i32) {
    ((wc - 1).div_euclid(SECTOR_W), (wr - 1).div_euclid(SECTOR_H))
}
thread_local! {
    static SECTOR_GEOM: RefCell<HashMap<(i32, i32), SectorGeom>> = RefCell::new(HashMap::new());
}

/// Build (once) per-sector border geometry: each group's fill `Path2d`, its
/// region hexes, the cached **interior** boundary stroke (same-sector edges),
/// and the list of hexes that touch an adjacent sector (for seam recompute).
fn build_sector_geom(sector: &SectorData) -> SectorGeom {
    use std::collections::HashSet;
    let mut groups: HashMap<&str, (String, Vec<(i32, i32)>)> = HashMap::new();
    for border in &sector.borders {
        if border.region.is_empty() {
            continue;
        }
        let entry = groups.entry(border_group_key(&border.allegiance)).or_insert_with(|| {
            let color = border
                .color
                .clone()
                .unwrap_or_else(|| allegiance_border_color(&border.allegiance).to_owned());
            (color, Vec::new())
        });
        entry.1.extend(border.region.iter().copied());
    }
    let groups = groups
        .into_iter()
        .filter_map(|(key, (color, region))| {
            let rset: HashSet<(i32, i32)> = region.iter().copied().collect();
            let fill = Path2d::new().ok()?;
            let interior_stroke = Path2d::new().ok()?;
            let mut seams = Vec::new();
            for &(wc, wr) in &region {
                // Inflate fill hexagons ~3% so neighbors overlap (no AA seam).
                let v0 = hex_vertex_r(wc, wr, 0, HEX_VR * 1.03);
                fill.move_to(v0.0, v0.1);
                for k in 1..6 {
                    let v = hex_vertex_r(wc, wr, k, HEX_VR * 1.03);
                    fill.line_to(v.0, v.1);
                }
                fill.close_path();
                // Same-sector edges resolve against this sector's region now
                // (cached interior stroke); cross-sector ones become seam
                // candidates resolved at combine against the neighbor's region.
                let hsec = hex_sector(wc, wr);
                for (nb, (va, vb)) in hex_neighbors(wc, wr) {
                    let nbsec = hex_sector(nb.0, nb.1);
                    if nbsec == hsec {
                        if !rset.contains(&nb) {
                            let (a, b) = (hex_vertex(wc, wr, va), hex_vertex(wc, wr, vb));
                            interior_stroke.move_to(a.0, a.1);
                            interior_stroke.line_to(b.0, b.1);
                        }
                    } else {
                        seams.push(SeamEdge {
                            nb_cell: nbsec,
                            nb,
                            a: hex_vertex(wc, wr, va),
                            b: hex_vertex(wc, wr, vb),
                        });
                    }
                }
            }
            Some(SectorGroup { key: key.to_owned(), color, fill, interior_stroke, rset, seams })
        })
        .collect();
    SectorGeom { groups }
}

/// Combine the (cached) per-sector geometry of the on-screen sectors into one
/// fill + stroke `Path2d` per polity group. Cheap: `add_path` for fills, and a
/// per-group combine: cached fills + interior strokes, with cross-sector seam
/// edges resolved against each neighbor sector's own region (no merged set).
fn build_border_geometry(sectors: &[&SectorData]) -> Vec<(String, Path2d, Path2d)> {
    SECTOR_GEOM.with(|cell| {
        // Phase 1: ensure each visible sector's geometry is built (cached once).
        {
            let mut cache = cell.borrow_mut();
            for sector in sectors {
                if let Some(loc) = sector.info.location {
                    cache.entry((loc.x, loc.y)).or_insert_with(|| build_sector_geom(sector));
                }
            }
        }
        // Phase 2 (read-only): combine cached fills + interior strokes per group,
        // and resolve each seam candidate against the neighbor sector's region —
        // no giant merged set, no per-hex rescan.
        let cache = cell.borrow();
        let in_region = |key: &str, co: (i32, i32), hex: (i32, i32)| -> bool {
            cache
                .get(&co)
                .is_some_and(|geom| geom.groups.iter().any(|g| g.key == key && g.rset.contains(&hex)))
        };
        let mut acc: HashMap<&str, (String, Path2d, Path2d)> = HashMap::new();
        for sector in sectors {
            let Some(loc) = sector.info.location else { continue };
            let Some(geom) = cache.get(&(loc.x, loc.y)) else { continue };
            for g in &geom.groups {
                let entry = acc
                    .entry(g.key.as_str())
                    .or_insert_with(|| (g.color.clone(), Path2d::new().unwrap(), Path2d::new().unwrap()));
                entry.1.add_path(&g.fill);
                entry.2.add_path(&g.interior_stroke); // cached same-sector edges
                // Seam edges: stroke only where the neighbor isn't in this
                // group's region in its sector (border ends at the seam).
                for seam in &g.seams {
                    if !in_region(&g.key, seam.nb_cell, seam.nb) {
                        entry.2.move_to(seam.a.0, seam.a.1);
                        entry.2.line_to(seam.b.0, seam.b.1);
                    }
                }
            }
        }
        acc.into_values().collect()
    })
}

/// Allegiance borders: filled interior + hex-edge outline, drawn from cached
/// world-space `Path2d`s under a view transform (see `border_group_key` for the
/// cross-sector polity grouping). `dpr` composes into the world→device transform
/// so strokes stay crisp on retina.
fn draw_micro_borders(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, dpr: f64, sectors: &[&SectorData], filled: bool) {
    let key = border_cache_key(sectors, filled);
    BORDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.as_ref().map(|c| c.key) != Some(key) {
            let t = now();
            let groups = build_border_geometry(sectors);
            let build_ms = now() - t;
            let hexes: usize = sectors.iter().flat_map(|s| &s.borders).map(|b| b.region.len()).sum();
            BORDER_STATS.with(|s| s.set(BorderStats { rebuilt: true, build_ms, groups: groups.len(), hexes }));
            *cache = Some(BorderCache { key, groups });
        } else {
            BORDER_STATS.with(|s| {
                let mut st = s.get();
                st.rebuilt = false;
                s.set(st);
            });
        }
        let Some(bc) = cache.as_ref() else { return };
        if bc.groups.is_empty() {
            return;
        }
        let ctx = &canvas.ctx;
        let s = view.scale;
        // World(parsec) → device: device = dpr · (w/2 + (p − center)·s).
        let a = dpr * s;
        let (e, f) = (dpr * (w / 2.0 - view.center.0 * s), dpr * (h / 2.0 - view.center.1 * s));
        let stroke_w = (0.10 * s).max(2.4) / s; // css width ÷ s (transform scales by s)
        ctx.save();
        let _ = ctx.set_transform(a, 0.0, 0.0, a, e, f);
        ctx.set_line_cap("round");
        ctx.set_line_join("round");
        ctx.set_line_width(stroke_w);
        for (color, fill, stroke) in &bc.groups {
            if filled {
                ctx.set_global_alpha(0.25); // FILL_ALPHA = 64/255
                ctx.set_fill_style_str(color);
                ctx.fill_with_path_2d(fill);
                ctx.set_global_alpha(1.0);
            }
            // Clip the outline to its region so only the inner half shows —
            // adjacent borders abut cleanly instead of double-stroking the seam.
            ctx.save();
            ctx.clip_with_path_2d(fill);
            ctx.set_stroke_style_str(color);
            ctx.stroke_with_path(stroke);
            ctx.restore();
        }
        ctx.restore();
    });
}

/// Border labels ("Third Imperium", …) — amber, at the label-position hex,
/// wrapped on spaces, horizontal (`microBorders.textColor`/`textStyle`).
fn draw_border_labels(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
    let Some(loc) = sector.info.location else {
        return;
    };
    let size = (0.5 * view.scale).clamp(11.0, 64.0);
    let font = format!("700 {}px {DEFAULT_FONT}", size as i32);
    for border in &sector.borders {
        let (Some(label), Some(pos)) = (&border.label, &border.label_position) else {
            continue;
        };
        let Some((col, row)) = parse_hex(pos) else {
            continue;
        };
        let (wc, wr) = world_hex(loc.x, loc.y, col, row);
        let (x, y) = view.to_screen(w, h, hex_parsec(wc, wr));
        if !on_screen(x, y, w, h, size * 4.0) {
            continue;
        }
        let lines: Vec<&str> = label.split_whitespace().collect();
        let top = y - (lines.len() as f64 - 1.0) * size * 0.55;
        for (i, line) in lines.iter().enumerate() {
            c.fill_text(line, x, top + i as f64 * size * 1.1, C_AMBER, &font, TextAlign::Center);
        }
    }
}

fn draw_routes(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
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

/// Border stroke color by allegiance — ported from `res/styles/otu.css`
/// (specific 4-char codes first, then the 2-char prefix fallback, then gray).
fn allegiance_border_color(a: &str) -> &'static str {
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


/// Straight sector/subsector boundary lines at every `step` parsecs (boundaries
/// sit half a hex outside the edge cells).
fn draw_grid_lines(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    step_col: i32,
    step_row: i32,
    color: &str,
    width: f64,
) {
    let (wc0, wc1, wr0, wr1) = visible_hex_range(view, w, h);
    let kx0 = (wc0 as f64 / step_col as f64).floor() as i32 - 1;
    let kx1 = (wc1 as f64 / step_col as f64).ceil() as i32 + 1;
    for k in kx0..=kx1 {
        let px = (k as f64 * step_col as f64 + 0.5) * PARSEC_SCALE_X as f64;
        let (sx, _) = view.to_screen(w, h, (px, 0.0));
        if sx >= -1.0 && sx <= w + 1.0 {
            c.stroke_polyline(&[(sx, 0.0), (sx, h)], color, width, false, &[]);
        }
    }
    let ky0 = (wr0 as f64 / step_row as f64).floor() as i32 - 1;
    let ky1 = (wr1 as f64 / step_row as f64).ceil() as i32 + 1;
    for k in ky0..=ky1 {
        let py = k as f64 * step_row as f64 + 0.5;
        let (_, sy) = view.to_screen(w, h, (0.0, py));
        if sy >= -1.0 && sy <= h + 1.0 {
            c.stroke_polyline(&[(0.0, sy), (w, sy)], color, width, false, &[]);
        }
    }
}

/// Big diagonal watermark labels — the reference rotates sector/subsector
/// names −50° and squishes them to 0.75 width (`sectorName.textStyle`).
const LABEL_ROT: f64 = -50.0 * std::f64::consts::PI / 180.0;
const LABEL_SCALE_X: f64 = 0.75;

/// Sector names: rotated watermark at sector centers (font 5.5 parsec).
fn draw_sector_names(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    sector_index: &HashMap<(i32, i32), String>,
) {
    let font_px = (5.5 * view.scale).clamp(10.0, 520.0);
    let font = format!("600 {}px {DEFAULT_FONT}", font_px as i32);
    for (&(sx, sy), name) in sector_index {
        let (cx, cy) = view.to_screen(w, h, sector_center(sx, sy));
        if !on_screen(cx, cy, w, h, font_px) {
            continue;
        }
        c.fill_text_rotated(name, cx, cy, "rgba(208,214,236,0.16)", &font, LABEL_ROT, LABEL_SCALE_X);
    }
}

/// Subsector names: rotated watermark at subsector centers (font 1.5 parsec).
fn draw_subsector_names(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sector: &SectorData) {
    let Some(loc) = sector.info.location else {
        return;
    };
    let font_px = (1.5 * view.scale).clamp(10.0, 260.0);
    let font = format!("600 {}px {DEFAULT_FONT}", font_px as i32);
    for ss in &sector.info.subsectors {
        let Some(letter) = ss.index.bytes().next() else {
            continue;
        };
        if !(b'A'..=b'P').contains(&letter) {
            continue;
        }
        let i = (letter - b'A') as i32;
        let (scol, srow) = (i % 4, i / 4);
        let wc = loc.x as f64 * SECTOR_W as f64 + scol as f64 * SUBSECTOR_W as f64 + 4.5;
        let wr = loc.y as f64 * SECTOR_H as f64 + srow as f64 * SUBSECTOR_H as f64 + 5.5;
        let (cx, cy) = view.to_screen(w, h, (wc * PARSEC_SCALE_X as f64, wr));
        if !on_screen(cx, cy, w, h, font_px) {
            continue;
        }
        c.fill_text_rotated(&ss.name, cx, cy, "rgba(206,200,228,0.22)", &font, LABEL_ROT, LABEL_SCALE_X);
    }
}

fn on_screen(x: f64, y: f64, w: f64, h: f64, pad: f64) -> bool {
    x >= -pad && x <= w + pad && y >= -pad && y <= h + pad
}

/// Cheap deterministic 2D hash for star placement (stable under pan).
fn hash2(a: i32, b: i32) -> u32 {
    let mut h = (a as u32).wrapping_mul(0x27d4_eb2d) ^ (b as u32).wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// Procedural star field in world space (pans with the map). Skipped when so
/// zoomed out that the cell count explodes.
fn draw_stars(c: &impl Canvas, view: &ViewState, w: f64, h: f64) {
    if view.scale < STAR_MIN_SCALE {
        return;
    }
    let (wc0, wc1, wr0, wr1) = visible_hex_range(view, w, h);
    if (wc1 - wc0) as i64 * (wr1 - wr0) as i64 > 45_000 {
        return; // too many cells to iterate cheaply when zoomed out
    }
    for wc in wc0..=wc1 {
        for wr in wr0..=wr1 {
            let hsh = hash2(wc, wr);
            if hsh % 7 != 0 {
                continue; // ~14% of cells host a star
            }
            let ox = ((hsh >> 3) & 0xff) as f64 / 255.0 - 0.5;
            let oy = ((hsh >> 11) & 0xff) as f64 / 255.0 - 0.5;
            let (px, py) = hex_parsec(wc, wr);
            let (sx, sy) = view.to_screen(w, h, (px + ox, py + oy));
            if !on_screen(sx, sy, w, h, 2.0) {
                continue;
            }
            let color = match (hsh >> 19) & 3 {
                0 => "rgba(170,180,205,0.35)",
                1 => "rgba(205,215,235,0.55)",
                2 => "rgba(230,235,250,0.75)",
                _ => "rgba(255,255,255,0.9)",
            };
            let r = if (hsh >> 27) & 1 == 0 { 0.7 } else { 1.1 };
            c.fill_circle(sx, sy, r, color);
        }
    }
}

// --- macro overlays ---------------------------------------------------------

fn draw_overlays(c: &impl Canvas, view: &ViewState, w: f64, h: f64, ov: &Overlays, opts: RenderOptions) {
    for v in &ov.rifts {
        draw_vector(c, view, w, h, v, C_RIFT, 1.0, false, &[]);
    }
    if opts.borders {
        for v in &ov.borders {
            draw_vector(c, view, w, h, v, C_BORDER, 1.5, false, &[]);
        }
    }
    if opts.routes {
        for v in &ov.routes {
            draw_vector(c, view, w, h, v, C_ROUTE, 1.3, true, &[6.0, 4.0]);
        }
    }
    // Region names ("THE IMPERIUM", …) on top.
    if opts.region_names {
        for v in &ov.borders {
            draw_region_label(c, view, w, h, v);
        }
    }
}

fn draw_vector(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    v: &VectorObject,
    color: &str,
    width: f64,
    force_open: bool,
    dash: &[f64],
) {
    for path in &v.paths {
        let pts: Vec<(f64, f64)> = path
            .points
            .iter()
            .map(|&(px, py)| {
                let wx = (px - v.origin.0) * v.scale.0;
                let wy = (py - v.origin.1) * v.scale.1;
                view.to_screen(w, h, (wx as f64 * PARSEC_SCALE_X as f64, wy as f64))
            })
            .collect();
        c.stroke_polyline(&pts, color, width, path.closed && !force_open, dash);
    }
}

fn draw_region_label(c: &impl Canvas, view: &ViewState, w: f64, h: f64, v: &VectorObject) {
    if v.name.is_empty() {
        return; // unnamed client-state regions get no major label
    }
    let Some((lx, ly)) = v.label else { return };
    let (sx, sy) = view.to_screen(w, h, (lx as f64 * PARSEC_SCALE_X as f64, ly as f64));
    if !on_screen(sx, sy, w, h, 200.0) {
        return;
    }
    let size = 15.0_f64;
    let font = format!("600 {}px system-ui, sans-serif", size as i32);
    let lines: Vec<&str> = v.name.split('\n').map(str::trim).collect();
    let top = sy - (lines.len() as f64 - 1.0) * size * 0.5;
    for (i, line) in lines.iter().enumerate() {
        c.fill_text(
            line,
            sx,
            top + i as f64 * size,
            "rgba(255,255,255,0.88)",
            &font,
            TextAlign::Center,
        );
    }
}

/// Capitals + homeworlds (`Overlays.labels`): a Wheat dot at the world hex with
/// a red name label offset by its `bias` (reference `WorldObject.Paint`).
fn draw_world_labels(c: &impl Canvas, view: &ViewState, w: f64, h: f64, ov: &Overlays) {
    let font = format!("600 13px {DEFAULT_FONT}");
    let r = (1.5 * view.scale).clamp(2.0, 6.0);
    for label in &ov.labels {
        let (x, y) = view.to_screen(w, h, hex_parsec(label.coord.x, label.coord.y));
        if !on_screen(x, y, w, h, 140.0) {
            continue;
        }
        c.fill_circle(x, y, r, "#f5deb3"); // Color.Wheat
        let (bx, by) = (label.bias.0 as f64, label.bias.1 as f64);
        let off = r + 4.0;
        let (lx, ly) = (x + bx * off, y + by * off);
        let align = if bx > 0.0 {
            TextAlign::Left
        } else if bx < 0.0 {
            TextAlign::Right
        } else {
            TextAlign::Center
        };
        let lines: Vec<&str> = label.name.split('\n').collect();
        let line_h = 14.0;
        let n = lines.len() as f64;
        // Anchor the text block on the dot's bias side (above if by<0, below if
        // by>0, centered if 0).
        let top = ly - (n - 1.0) * line_h * if by < 0.0 { 1.0 } else if by > 0.0 { 0.0 } else { 0.5 };
        for (i, line) in lines.iter().enumerate() {
            c.fill_text(line, lx, top + i as f64 * line_h, "#e8636f", &font, align);
        }
    }
}

// --- grid + worlds ----------------------------------------------------------

// Persistent per-sector hex-grid geometry: one `Path2d` of all 1280 hex
// outlines in world (parsec) coords, built once per sector. Same trick as the
// border cache — the old grid issued a separate `stroke()` per on-screen
// hexagon (thousands of wasm→JS crossings per frame, the zoomed-in hot layer);
// now we stroke cached world-space paths under one view transform. (Clear on
// milieu switch, like `SECTOR_GEOM`.)
thread_local! {
    static GRID_GEOM: RefCell<HashMap<(i32, i32), Path2d>> = RefCell::new(HashMap::new());
}

/// Build (once) the full hex-grid outline `Path2d` for one sector, in world
/// coords (so it composes with the world→device transform like the borders).
fn build_grid_geom(loc: (i32, i32)) -> Path2d {
    let p = Path2d::new().unwrap();
    for col in 1..=SECTOR_W {
        for row in 1..=SECTOR_H {
            let (wc, wr) = (loc.0 * SECTOR_W + col, loc.1 * SECTOR_H + row);
            let v0 = hex_vertex(wc, wr, 0);
            p.move_to(v0.0, v0.1);
            for k in 1..6 {
                let v = hex_vertex(wc, wr, k);
                p.line_to(v.0, v.1);
            }
            p.close_path();
        }
    }
    p
}

/// Does this sector's bounding box overlap the viewport (+~1 hex margin)? Used
/// to skip off-screen sectors in the `sectors` slice so we don't stroke their
/// whole grids.
fn sector_in_viewport(loc: (i32, i32), view: &ViewState, w: f64, h: f64) -> bool {
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

/// Per-parsec hex grid, drawn from cached per-sector world-space `Path2d`s under
/// one view transform — a handful of `add_path` + a single `stroke`, instead of
/// a `stroke()` per on-screen hexagon.
fn draw_hex_grid(
    canvas: &Canvas2d,
    view: &ViewState,
    w: f64,
    h: f64,
    dpr: f64,
    sector_index: &HashMap<(i32, i32), String>,
) {
    let s = view.scale;
    if s / 3f64.sqrt() < 2.0 {
        return; // hexes too small to read
    }
    // Draw the grid for every *charted* sector overlapping the viewport (from
    // the index, not the loaded set) so it shows regardless of world-data load
    // state and never tiles the uncharted void.
    let combined = Path2d::new().unwrap();
    let mut any = false;
    GRID_GEOM.with(|cache| {
        let mut cache = cache.borrow_mut();
        for cell in visible_sectors(view, w, h) {
            if !sector_index.contains_key(&cell) || !sector_in_viewport(cell, view, w, h) {
                continue;
            }
            let g = cache.entry(cell).or_insert_with(|| build_grid_geom(cell));
            combined.add_path(g);
            any = true;
        }
    });
    if !any {
        return;
    }
    let ctx = &canvas.ctx;
    // World(parsec) → device: device = dpr · (w/2 + (p − center)·s), uniform.
    let a = dpr * s;
    let (e, f) = (dpr * (w / 2.0 - view.center.0 * s), dpr * (h / 2.0 - view.center.1 * s));
    ctx.save();
    let _ = ctx.set_transform(a, 0.0, 0.0, a, e, f);
    ctx.set_line_width(1.0 / s); // ~1 css px (the transform scales by s)
    ctx.set_stroke_style_str("rgba(130,150,190,0.22)");
    ctx.stroke_with_path(&combined);
    ctx.restore();
}

/// Dot-tier disc radius (parsec) and zone-ring radius (parsec) — constant in
/// world space, so the dot geometry can be cached and drawn under the view
/// transform.
const DOT_R: f64 = 0.1 * CONTENT_SCALE;
const ZONE_R: f64 = 0.4;

/// Cached per-sector "dot tier" geometry (scale < `WORLD_BASIC_SCALE`, no text):
/// world discs + travel-zone rings grouped by color into `Path2d`s in world
/// coords. Built once per sector (per `more_colors` setting) so a zoomed-out
/// frame with thousands of worlds issues a few `fill`/`stroke`s instead of a
/// `fill_circle`/`stroke_arc` per world. *(Clear on milieu switch.)*
struct SectorDots {
    more_colors: bool,
    discs: Vec<(String, Path2d)>,
    outlines: Vec<(String, Path2d)>,
    zones: Vec<(String, Path2d)>,
}
thread_local! {
    static SECTOR_DOTS: RefCell<HashMap<(i32, i32), SectorDots>> = RefCell::new(HashMap::new());
}

fn build_sector_dots(sector: &SectorData, more_colors: bool) -> SectorDots {
    use std::f64::consts::PI;
    let mut discs: HashMap<String, Path2d> = HashMap::new();
    let mut outlines: HashMap<String, Path2d> = HashMap::new();
    let mut zones: HashMap<String, Path2d> = HashMap::new();
    if let Some(loc) = sector.info.location {
        let add_circle = |map: &mut HashMap<String, Path2d>, color: &str, cx: f64, cy: f64| {
            let p = map.entry(color.to_owned()).or_insert_with(|| Path2d::new().unwrap());
            p.move_to(cx + DOT_R, cy);
            let _ = p.arc(cx, cy, DOT_R, 0.0, 2.0 * PI);
        };
        for world in &sector.worlds {
            let Some((col, row)) = parse_hex(&world.hex) else { continue };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (cx, cy) = hex_parsec(wc, wr);
            // Travel-zone open-bottom arc (behind the disc).
            let zc = match world.zone.as_str() { "A" => Some(C_AMBER), "R" => Some(C_RED), _ => None };
            if let Some(zc) = zc {
                let (a0, a1) = (PI - 0.384, 2.0 * PI + 0.384);
                let p = zones.entry(zc.to_owned()).or_insert_with(|| Path2d::new().unwrap());
                p.move_to(cx + ZONE_R * a0.cos(), cy + ZONE_R * a0.sin());
                let _ = p.arc(cx, cy, ZONE_R, a0, a1);
            }
            let (fill, outline) = world_colors(world, more_colors);
            add_circle(&mut discs, fill, cx, cy);
            if let Some(oc) = outline {
                add_circle(&mut outlines, oc, cx, cy);
            }
        }
    }
    SectorDots {
        more_colors,
        discs: discs.into_iter().collect(),
        outlines: outlines.into_iter().collect(),
        zones: zones.into_iter().collect(),
    }
}

/// Dot-tier worlds (scale < `WORLD_BASIC_SCALE`): batched discs + zone rings
/// from the per-sector cache, drawn under one view transform.
fn draw_world_dots(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, dpr: f64, sectors: &[&SectorData], more_colors: bool) {
    let s = view.scale;
    let mut discs: HashMap<String, Path2d> = HashMap::new();
    let mut outlines: HashMap<String, Path2d> = HashMap::new();
    let mut zones: HashMap<String, Path2d> = HashMap::new();
    let merge = |dst: &mut HashMap<String, Path2d>, src: &[(String, Path2d)]| {
        for (c, p) in src {
            dst.entry(c.clone()).or_insert_with(|| Path2d::new().unwrap()).add_path(p);
        }
    };
    SECTOR_DOTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        for sector in sectors {
            let Some(loc) = sector.info.location else { continue };
            if !sector_in_viewport((loc.x, loc.y), view, w, h) {
                continue;
            }
            let dots = cache.entry((loc.x, loc.y)).or_insert_with(|| build_sector_dots(sector, more_colors));
            if dots.more_colors != more_colors {
                *dots = build_sector_dots(sector, more_colors); // toggle changed disc colors
            }
            merge(&mut zones, &dots.zones);
            merge(&mut discs, &dots.discs);
            merge(&mut outlines, &dots.outlines);
        }
    });
    let ctx = &canvas.ctx;
    let a = dpr * s;
    let (e, f) = (dpr * (w / 2.0 - view.center.0 * s), dpr * (h / 2.0 - view.center.1 * s));
    let cs = s * CONTENT_SCALE;
    ctx.save();
    let _ = ctx.set_transform(a, 0.0, 0.0, a, e, f);
    // Zones first (behind), then disc fills, then vacuum outlines. Line widths
    // are css px ÷ s (the transform scales by s).
    ctx.set_line_width(((0.03 * cs).max(1.5)) / s);
    for (color, path) in &zones {
        ctx.set_stroke_style_str(color);
        ctx.stroke_with_path(path);
    }
    for (color, path) in &discs {
        ctx.set_fill_style_str(color);
        ctx.fill_with_path_2d(path);
    }
    ctx.set_line_width(((0.02 * cs).max(1.0)) / s);
    for (color, path) in &outlines {
        ctx.set_stroke_style_str(color);
        ctx.stroke_with_path(path);
    }
    ctx.restore();
}

/// Faithful port of the reference `DrawWorld` text layout, drawn in
/// **state-batched passes**: the disc/zone/outline geometry comes from the
/// cached dot paths (`draw_world_dots`), and here every glyph kind (hex#,
/// starport, gas giant, bases, UWP, allegiance, name) is drawn as one pass that
/// sets the canvas font/fill/align **once** then loops `fillText` over all
/// on-screen worlds — instead of re-setting that state per glyph per world.
/// Offsets and font sizes are in parsec units (× scale → px); `cs = s ·
/// CONTENT_SCALE` sizes glyphs to fill the hex while layout offsets use true `s`.
fn draw_world_glyphs(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, sectors: &[&SectorData]) {
    let ctx = &canvas.ctx;
    let s = view.scale;
    let poster = s >= WORLD_FULL_SCALE; // poster vs atlas positions
    let show_uwp = s >= WORLD_UWP_SCALE;
    let cs = s * CONTENT_SCALE;

    // Layout offsets (parsec), poster vs atlas (RenderContext / Stylesheet).
    let (sp_y, uwp_y, name_y) = if poster { (-0.225, 0.225, 0.37) } else { (-0.24, 0.24, 0.40) };
    let (gg_x, gg_y) = if poster { (0.25, -0.18) } else { (0.225, -0.125) };
    let base_x = if poster { -0.25 } else { -0.225 };
    let zone_r = 0.4 * s; // (only used to size the off-screen cull margin)

    // Font sizes (parsec → px), porting Stylesheet's fontScale.
    let font_scale = if s <= 96.0 { 1.0 } else { 96.0 / s.min(192.0) };
    let name_pt = (if poster { 0.15 * font_scale } else { 0.2 }) * cs;
    let uwp_pt = 0.13 * font_scale * cs;
    let hex_pt = 0.10 * font_scale * cs;
    let name_font = format!("700 {}px {DEFAULT_FONT}", name_pt.max(7.0) as i32);
    let uwp_font = format!("500 {}px {DEFAULT_FONT}", uwp_pt.max(7.0) as i32);
    let hex_font = format!("{}px {DEFAULT_FONT}", hex_pt.max(6.0) as i32);
    let glyph_pt = (if poster { 0.15 * font_scale } else { 0.175 }) * cs;
    let glyph_font = format!("{}px {DEFAULT_FONT}", glyph_pt.max(7.0) as i32);
    // Base slots (left side); bottom slot rises when the UWP needs the room.
    let base_top_y = if poster { -0.18 } else { -0.125 };
    let base_bottom_y = if show_uwp { 0.1 } else if poster { 0.18 } else { 0.125 };

    let pad = zone_r + name_pt * 3.0 + 12.0;

    // Collect on-screen worlds once (screen coords), shared by every pass.
    let mut vis: Vec<(&World, f64, f64)> = Vec::new();
    for sector in sectors {
        let Some(loc) = sector.info.location else { continue };
        for world in &sector.worlds {
            let Some((col, row)) = parse_hex(&world.hex) else { continue };
            let (wc, wr) = world_hex(loc.x, loc.y, col, row);
            let (x, y) = view.to_screen(w, h, hex_parsec(wc, wr));
            if !on_screen(x, y, w, h, pad) {
                continue;
            }
            vis.push((world, x, y));
        }
    }
    if vis.is_empty() {
        return;
    }

    // Text is centered vertically at its y; set once for every pass below.
    ctx.set_text_baseline("middle");

    // ── Hex number (top, just inside the top edge — reference TopCenter).
    ctx.set_font(&hex_font);
    ctx.set_text_align("center");
    ctx.set_fill_style_str("#9aa3b8");
    let hex_dy = -0.5 * s + hex_pt * 0.55;
    for (world, x, y) in &vis {
        let _ = ctx.fill_text(&world.hex, *x, *y + hex_dy);
    }

    // ── Starport class (above the disc). Same font as names (700, name_pt).
    ctx.set_font(&name_font);
    ctx.set_fill_style_str("#e9eef9");
    for (world, x, y) in &vis {
        if let Some(sp) = world.uwp.chars().next() {
            if sp != '?' {
                let _ = ctx.fill_text(sp.encode_utf8(&mut [0u8; 4]), *x, *y + sp_y * s);
            }
        }
    }

    // ── Gas giant (upper-right): filled discs batched into one path; Saturn
    // ring (only when zoomed past the UWP threshold) stroked per giant.
    {
        let r = (0.05 * cs).max(1.0);
        let disc = Path2d::new().unwrap();
        let mut any = false;
        let has_gg = |wld: &World| wld.pbg.as_bytes().get(2).is_some_and(|&b| b > b'0' && b != b'?');
        for (world, x, y) in &vis {
            if has_gg(world) {
                let (gx, gy) = (*x + gg_x * s, *y + gg_y * s);
                disc.move_to(gx + r, gy);
                let _ = disc.arc(gx, gy, r, 0.0, std::f64::consts::TAU);
                any = true;
            }
        }
        if any {
            ctx.set_fill_style_str("#cfd6e6");
            ctx.fill_with_path_2d(&disc);
            if show_uwp {
                ctx.set_stroke_style_str("#cfd6e6");
                ctx.set_line_width((r / 4.0).max(0.6));
                for (world, x, y) in &vis {
                    if has_gg(world) {
                        let (gx, gy) = (*x + gg_x * s, *y + gg_y * s);
                        ctx.begin_path();
                        let _ = ctx.ellipse(gx, gy, r * 1.75, r * 0.4, -0.5236, 0.0, std::f64::consts::TAU);
                        ctx.stroke();
                    }
                }
            }
        }
    }

    // ── Bases (left side) as classic glyphs. Font/align set once; fill toggles
    // only between white and red (red is sparse), so track the last color.
    ctx.set_font(&glyph_font);
    ctx.set_text_align("center");
    let mut last = "";
    let bx = base_x * s;
    for (world, x, y) in &vis {
        let mut chars = world.bases.chars();
        let mut bottom_used = false;
        if let Some(c0) = chars.next() {
            if let Some(g) = glyph::base_glyph(&world.allegiance, c0) {
                bottom_used = g.bias == glyph::Bias::Bottom;
                let col = if g.highlight { C_RED } else { "#e9eef9" };
                if col != last { ctx.set_fill_style_str(col); last = col; }
                let gy = if bottom_used { base_bottom_y } else { base_top_y } * s;
                let _ = ctx.fill_text(g.chars, *x + bx, *y + gy);
            }
        }
        if let Some(c1) = chars.next() {
            if let Some(g) = glyph::base_glyph(&world.allegiance, c1) {
                let bottom = !bottom_used;
                let col = if g.highlight { C_RED } else { "#e9eef9" };
                if col != last { ctx.set_fill_style_str(col); last = col; }
                let gy = if bottom { base_bottom_y } else { base_top_y } * s;
                let _ = ctx.fill_text(g.chars, *x + bx, *y + gy);
            }
        }
    }

    // ── UWP (above name), only past the UWP scale threshold.
    if show_uwp {
        ctx.set_font(&uwp_font);
        ctx.set_text_align("center");
        ctx.set_fill_style_str("#c9d2e4");
        for (world, x, y) in &vis {
            let _ = ctx.fill_text(&world.uwp, *x, *y + uwp_y * s);
        }
    }

    // ── Allegiance code (e.g. NaHu) to the right of the disc, when zoomed in.
    if s >= ALLEGIANCE_MIN_SCALE {
        ctx.set_font(&uwp_font);
        ctx.set_text_align("left");
        ctx.set_fill_style_str("#aab3c8");
        for (world, x, y) in &vis {
            if !world.allegiance.is_empty() && world.allegiance != "--" {
                let _ = ctx.fill_text(&world.allegiance, *x + 0.20 * s, *y + 0.08 * s);
            }
        }
    }

    // ── World name (bottom). High-pop (≥1e9) in ALL CAPS, capitals in red —
    // the reference's `IsHi` uppercase + capital highlight. Fill toggles only
    // for the sparse capitals, so track the last color rather than re-setting.
    ctx.set_font(&name_font);
    ctx.set_text_align("center");
    let mut last = "";
    let name_dy = name_y * s;
    for (world, x, y) in &vis {
        let hi_pop = world.uwp.as_bytes().get(4).copied().and_then(ehex).is_some_and(|p| p >= 9);
        let is_capital = world.codes().any(|c| matches!(c, "Cp" | "Cs" | "Cx" | "Capital"));
        let col = if is_capital { "#e8636f" } else { "#e9eef9" };
        if col != last { ctx.set_fill_style_str(col); last = col; }
        if hi_pop {
            let _ = ctx.fill_text(&world.name.to_uppercase(), *x, *y + name_dy);
        } else {
            let _ = ctx.fill_text(&world.name, *x, *y + name_dy);
        }
    }
}

/// The reference map renders in Arial (Bold for world names). Matching it
/// keeps text the same width/weight — `system-ui` (San Francisco on macOS) is
/// narrower and reads smaller at the same px size.
const DEFAULT_FONT: &str = "Arial, 'Helvetica Neue', Helvetica, sans-serif";

/// Hex (ehex) digit value: 0-9, A=10 … (Traveller extended hex).
fn ehex(c: u8) -> Option<i32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as i32),
        b'A'..=b'Z' => Some((c - b'A') as i32 + 10),
        _ => None,
    }
}

/// World disc (fill, optional outline), porting `Stylesheet.WorldColors`
/// detail-color mode (color by trade classification).
fn world_colors(world: &World, more_colors: bool) -> (&'static str, Option<&'static str>) {
    let has = |code: &str| world.codes().any(|c| c == code);
    let atmo = world.uwp.as_bytes().get(2).copied().and_then(ehex);
    let hydro = world.uwp.as_bytes().get(3).copied().and_then(ehex);
    if !more_colors {
        // Plain mode: water worlds blue, everything else white (no trade-class
        // tints) — the reference's "More World Colors" off.
        let water = hydro.is_some_and(|h| h > 0)
            && atmo.is_some_and(|a| (2..=9).contains(&a) || (13..=15).contains(&a));
        let vacuum = has("Va") || atmo == Some(0);
        return if vacuum {
            ("#000000", Some("#ffffff"))
        } else if water {
            (C_WATER, None)
        } else {
            (C_DRY, None)
        };
    }
    let (ag, ri, ind) = (has("Ag"), has("Ri"), has("In"));
    let vacuum = has("Va") || atmo == Some(0);
    let water = hydro.is_some_and(|h| h > 0)
        && atmo.is_some_and(|a| (2..=9).contains(&a) || (13..=15).contains(&a));

    if ag && ri {
        (C_AMBER, None)
    } else if ag {
        ("#048104", None) // Green
    } else if ri {
        ("#a000a0", None) // Purple (Rich)
    } else if ind {
        ("#888888", None) // Gray (Industrial)
    } else if atmo.is_some_and(|a| a > 10) {
        ("#cc6626", None) // Rust (dense/exotic atmosphere)
    } else if vacuum {
        ("#000000", Some("#ffffff")) // Black disc, white outline
    } else if water {
        (C_WATER, None) // DeepSkyBlue
    } else {
        (C_DRY, None) // White
    }
}
