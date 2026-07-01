//! Micro (per-sector) allegiance borders: filled interiors + hex-edge outlines,
//! grouped by polity across all loaded sectors and cached as world-space
//! [`Geometry`] so a pan re-transforms (via `fill_geometry`/`stroke_geometry`)
//! instead of re-emitting tens of thousands of path calls per frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tmap_core::dto::SectorData;
use tmap_core::spline::cardinal_spline_beziers;

use super::common::{
    allegiance_border_color, hex_neighbors, hex_sector, hex_vertex, hex_vertex_r, ViewState, HEX_VR,
};
use super::now;
use crate::canvas::{Affine, Canvas, Geometry, PathBuilder, StrokeStyle};

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
    groups: Vec<BorderGroup>,
}
/// One polity's border geometry for the current sector set. The region fills are
/// kept as the **individual per-sector** paths (shared `Rc`s of the cached
/// `SECTOR_GEOM` fills, so their materialized `Path2d`s persist) rather than one
/// concatenated geometry — the backend unions them with native `add_path` at draw
/// instead of re-rasterizing a command list every frame. The stroke is the
/// combined boundary (interior edges + resolved cross-sector seams); it's small,
/// so rebuilding it per set-change is cheap.
struct BorderGroup {
    color: String,
    fills: Vec<Rc<Geometry>>,
    stroke: Geometry,
}
thread_local! {
    static BORDER_CACHE: RefCell<Option<BorderCache>> = const { RefCell::new(None) };
}

/// Last-frame border render stats, surfaced in the perf HUD so we can see
/// whether a spike is a cache *rebuild* (expensive) or just the cached redraw.
#[derive(Default, Clone, Copy)]
pub(crate) struct BorderStats {
    pub(crate) rebuilt: bool,
    pub(crate) build_ms: f64,
    pub(crate) groups: usize,
    pub(crate) hexes: usize,
}
thread_local! {
    pub(crate) static BORDER_STATS: std::cell::Cell<BorderStats> =
        const { std::cell::Cell::new(BorderStats { rebuilt: false, build_ms: 0.0, groups: 0, hexes: 0 }) };
}

/// Hash of the on-screen *bordered* sectors (+ fill flag). The cache rebuilds
/// only when this set changes (sectors stream in / zoom changes the set), not on
/// every pan frame — between changes the cached `Path2d`s are just re-transformed.
fn border_cache_key(sectors: &[&SectorData], filled: bool, curved: bool) -> u64 {
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
    curved.hash(&mut h); // hex vs curved geometry are cached separately
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
    fill: Rc<Geometry>,
    /// Boundary edges to **same-sector** non-region neighbors — determined by
    /// this sector's region alone, so cached once (the bulk of the stroke).
    interior_stroke: Geometry,
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

thread_local! {
    static SECTOR_GEOM: RefCell<HashMap<(i32, i32), SectorGeom>> = RefCell::new(HashMap::new());
}

/// Clear the cached border geometry (milieu switch).
pub(crate) fn clear_border_caches() {
    SECTOR_GEOM.with(|c| c.borrow_mut().clear());
    BORDER_CACHE.with(|c| *c.borrow_mut() = None);
}

/// Build (once) per-sector border geometry: each group's fill `Path2d`, its
/// region hexes, the cached **interior** boundary stroke (same-sector edges),
/// and the list of hexes that touch an adjacent sector (for seam recompute).
/// Accumulator while grouping a sector's borders by polity: the resolved stroke/
/// fill color plus the union of every region hex for that group.
struct GroupAccum {
    color: String,
    region: Vec<(i32, i32)>,
}

fn build_sector_geom(sector: &SectorData) -> SectorGeom {
    use std::collections::HashSet;
    let mut groups: HashMap<&str, GroupAccum> = HashMap::new();
    for border in &sector.borders {
        if border.region.is_empty() {
            continue;
        }
        let entry = groups
            .entry(border_group_key(&border.allegiance))
            .or_insert_with(|| GroupAccum {
                color: border
                    .color
                    .clone()
                    .unwrap_or_else(|| allegiance_border_color(&border.allegiance).to_owned()),
                region: Vec::new(),
            });
        entry.region.extend(border.region.iter().copied());
    }
    let groups = groups
        .into_iter()
        .map(|(key, GroupAccum { color, region })| {
            let rset: HashSet<(i32, i32)> = region.iter().copied().collect();
            let fill = PathBuilder::new();
            let interior_stroke = PathBuilder::new();
            let mut seams = Vec::new();
            for &(wc, wr) in &region {
                // Inflate fill hexagons ~3% so neighbors overlap (no AA seam).
                let v0 = hex_vertex_r(wc, wr, 0, HEX_VR * 1.03);
                fill.move_to(v0.0, v0.1);
                for k in 1..6 {
                    let v = hex_vertex_r(wc, wr, k, HEX_VR * 1.03);
                    fill.line_to(v.0, v.1);
                }
                fill.close();
                // Same-sector edges resolve against this sector's region now
                // (cached interior stroke); cross-sector ones become seam
                // candidates resolved at combine against the neighbor's region.
                let hsec = hex_sector(wc, wr);
                for (nb, (va, vb)) in hex_neighbors(wc, wr) {
                    // A neighbor already in THIS region is interior, full stop —
                    // even when it lives in an adjacent sector. Authors draw some
                    // polity regions to overlap across the seam (the reference's
                    // `border.Path.Contains(neighbor)` checks the border's own
                    // path, which may list out-of-sector hexes), so this must be
                    // tested before the same-sector/seam split. Otherwise such an
                    // edge becomes a seam candidate, gets resolved against the
                    // neighbor sector's separately-encoded region (which doesn't
                    // list the hex), and is stroked as a false "territory seam".
                    if rset.contains(&nb) {
                        continue;
                    }
                    let nbsec = hex_sector(nb.0, nb.1);
                    if nbsec == hsec {
                        let (a, b) = (hex_vertex(wc, wr, va), hex_vertex(wc, wr, vb));
                        interior_stroke.move_to(a.0, a.1);
                        interior_stroke.line_to(b.0, b.1);
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
            SectorGroup {
                key: key.to_owned(),
                color,
                fill: Rc::new(fill.finish()),
                interior_stroke: interior_stroke.finish(),
                rset,
                seams,
            }
        })
        .collect();
    SectorGeom { groups }
}

/// Combine the (cached) per-sector geometry of the on-screen sectors into one
/// fill + stroke `Path2d` per polity group. Cheap: `add_path` for fills, and a
/// per-group combine: cached fills + interior strokes, with cross-sector seam
/// edges resolved against each neighbor sector's own region (no merged set).
fn build_border_geometry(sectors: &[&SectorData]) -> Vec<BorderGroup> {
    SECTOR_GEOM.with(|cell| {
        // Phase 1: ensure each visible sector's geometry is built (cached once).
        {
            let mut cache = cell.borrow_mut();
            for sector in sectors {
                if let Some(loc) = sector.info.location {
                    cache
                        .entry((loc.x, loc.y))
                        .or_insert_with(|| build_sector_geom(sector));
                }
            }
        }
        // Phase 2 (read-only): combine cached fills + interior strokes per group,
        // and resolve each seam candidate against the neighbor sector's region —
        // no giant merged set, no per-hex rescan.
        let cache = cell.borrow();
        // Resolve a seam edge against the neighbor sector's region. A seam
        // candidate only exists where this sector's OWN region does NOT include
        // the neighbor hex (see `build_sector_geom` — region overlap across a
        // seam is caught there and never becomes a seam), so it's a genuine
        // polity boundary unless the neighbor sector continues the same region.
        // Stroke it UNLESS the neighbor sector is built and lists that hex in the
        // same group — i.e. an unbuilt/uncharted neighbor still closes the border
        // (an undetailed neighbor in e.g. the Interstellar Wars milieu must not
        // leave the Imperium border cut off). When the neighbor later streams in,
        // the cache rebuilds and re-resolves against its real region.
        let neighbor_strokes = |key: &str, co: (i32, i32), hex: (i32, i32)| -> bool {
            match cache.get(&co) {
                None => true, // neighbor not built → close the border at this edge
                Some(geom) => !geom
                    .groups
                    .iter()
                    .any(|g| g.key == key && g.rset.contains(&hex)),
            }
        };
        let mut acc: HashMap<&str, (String, Vec<Rc<Geometry>>, PathBuilder)> = HashMap::new();
        for sector in sectors {
            let Some(loc) = sector.info.location else {
                continue;
            };
            let Some(geom) = cache.get(&(loc.x, loc.y)) else {
                continue;
            };
            for g in &geom.groups {
                let entry = acc
                    .entry(g.key.as_str())
                    .or_insert_with(|| (g.color.clone(), Vec::new(), PathBuilder::new()));
                entry.1.push(Rc::clone(&g.fill)); // share the cached per-sector fill
                entry.2.add(&g.interior_stroke); // cached same-sector edges
                                                 // Seam edges: stroke only where the neighbor sector is built AND
                                                 // the neighbor hex isn't in this group's region (border ends at
                                                 // the seam). An unbuilt neighbor is left un-stroked, not bordered.
                for seam in &g.seams {
                    if neighbor_strokes(&g.key, seam.nb_cell, seam.nb) {
                        entry.2.move_to(seam.a.0, seam.a.1);
                        entry.2.line_to(seam.b.0, seam.b.1);
                    }
                }
            }
        }
        acc.into_values()
            .map(|(color, fills, stroke)| BorderGroup {
                color,
                fills,
                stroke: stroke.finish(),
            })
            .collect()
    })
}

// ── Curved (FASA/Candy) borders ─────────────────────────────────────────────

/// A boundary edge as both endpoints' quantized vertex key + world point.
type Edge = ((i64, i64), (f64, f64), (i64, i64), (f64, f64));

/// Quantize a world vertex so two hexes' shared corner (geometrically identical,
/// possibly off by float rounding) dedupes to one graph node.
fn vkey(p: (f64, f64)) -> (i64, i64) {
    ((p.0 * 4096.0).round() as i64, (p.1 * 4096.0).round() as i64)
}

/// A traced boundary loop and whether it closed on itself (a closed polity
/// outline) or ran into the loaded-sector frontier (open).
struct BorderLoop {
    points: Vec<(f64, f64)>,
    closed: bool,
}

/// Stitch a bag of boundary edges into ordered loops by following shared vertices.
/// Degree-2 vertices give clean loops; at rare pinch points (degree 4) we just take
/// any still-unused edge.
fn stitch_loops(edges: &[Edge]) -> Vec<BorderLoop> {
    let mut adj: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        adj.entry(e.0).or_default().push(i);
        adj.entry(e.2).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        let start_v = edges[start].0;
        let mut cur_v = start_v;
        let mut e_idx = start;
        let mut points = Vec::new();
        let closed = loop {
            used[e_idx] = true;
            let e = &edges[e_idx];
            let (this_pt, other_v) = if e.0 == cur_v { (e.1, e.2) } else { (e.3, e.0) };
            points.push(this_pt);
            cur_v = other_v;
            if cur_v == start_v {
                break true;
            }
            match adj
                .get(&cur_v)
                .and_then(|es| es.iter().copied().find(|&j| !used[j]))
            {
                Some(j) => e_idx = j,
                None => break false,
            }
        };
        if points.len() >= 2 {
            loops.push(BorderLoop { points, closed });
        }
    }
    loops
}

/// Cardinal-spline `lp` into `path` (move + béziers, close if the loop is closed).
fn spline_into(path: &PathBuilder, lp: &BorderLoop, tension: f64) {
    if lp.points.len() < 2 {
        return;
    }
    path.move_to(lp.points[0].0, lp.points[0].1);
    for [c1, c2, end] in cardinal_spline_beziers(&lp.points, tension, lp.closed) {
        path.bezier_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
    }
    if lp.closed {
        path.close();
    }
}

/// Curved-border geometry (`microBorderStyle == Curve`): combine each polity
/// group's region across the visible sectors, trace its boundary edges into loops,
/// and cardinal-spline them into a fill (tension 0.5) + stroke (0.6) `Path2d`.
/// Combining across sectors means a seam interior to a polity isn't a boundary
/// edge, so no per-sector seam resolution is needed (curve styles are rare, so the
/// hex path's per-frame caching concern doesn't apply — this rebuilds on the same
/// sector-set change as the hex path).
fn build_curved_geometry(sectors: &[&SectorData]) -> Vec<BorderGroup> {
    use std::collections::HashSet;
    let mut regions: HashMap<&str, HashSet<(i32, i32)>> = HashMap::new();
    let mut colors: HashMap<&str, String> = HashMap::new();
    for sector in sectors {
        for border in &sector.borders {
            if border.region.is_empty() {
                continue;
            }
            let key = border_group_key(&border.allegiance);
            colors.entry(key).or_insert_with(|| {
                border
                    .color
                    .clone()
                    .unwrap_or_else(|| allegiance_border_color(&border.allegiance).to_owned())
            });
            regions
                .entry(key)
                .or_default()
                .extend(border.region.iter().copied());
        }
    }
    regions
        .into_iter()
        .map(|(key, rset)| {
            let mut edges: Vec<Edge> = Vec::new();
            for &(wc, wr) in &rset {
                for (nb, (va, vb)) in hex_neighbors(wc, wr) {
                    if rset.contains(&nb) {
                        continue; // interior edge (incl. across a sector seam)
                    }
                    let a = hex_vertex(wc, wr, va);
                    let b = hex_vertex(wc, wr, vb);
                    edges.push((vkey(a), a, vkey(b), b));
                }
            }
            let loops = stitch_loops(&edges);
            let fill = PathBuilder::new();
            let stroke = PathBuilder::new();
            for lp in &loops {
                spline_into(&fill, lp, 0.5);
                spline_into(&stroke, lp, 0.6);
            }
            BorderGroup {
                color: colors.get(key).cloned().unwrap_or_default(),
                fills: vec![Rc::new(fill.finish())],
                stroke: stroke.finish(),
            }
        })
        .collect()
}

/// Allegiance borders: filled interior + hex-edge outline, drawn from cached
/// world-space `Path2d`s under a view transform (see `border_group_key` for the
/// cross-sector polity grouping). `dpr` composes into the world→device transform
/// so strokes stay crisp on retina.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_micro_borders(
    c: &impl Canvas,
    view: &ViewState,
    w: f64,
    h: f64,
    dpr: f64,
    sectors: &[&SectorData],
    filled: bool,
    micro_override: Option<&str>,
    curved: bool,
    taper: bool,
) {
    let key = border_cache_key(sectors, filled, curved);
    BORDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.as_ref().map(|c| c.key) != Some(key) {
            let t = now();
            let groups = if curved {
                build_curved_geometry(sectors)
            } else {
                build_border_geometry(sectors)
            };
            let build_ms = now() - t;
            let hexes: usize = sectors
                .iter()
                .flat_map(|s| &s.borders)
                .map(|b| b.region.len())
                .sum();
            BORDER_STATS.with(|s| {
                s.set(BorderStats {
                    rebuilt: true,
                    build_ms,
                    groups: groups.len(),
                    hexes,
                })
            });
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
        let s = view.scale;
        // World(parsec) → device: device = dpr · (w/2 + (p − center)·s).
        let m = Affine::scale_translate(
            dpr * s,
            dpr * (w / 2.0 - view.center.0 * s),
            dpr * (h / 2.0 - view.center.1 * s),
        );
        // Width is in world (parsec) units — the transform scales it by `s`. Hex:
        // our 0.10 parsec (min 2.4 px). Curved (FASA/Candy): the reference
        // `borderPenWidth = 0.16·penScale`, which Candy tapers to ÷4 past scale 32
        // (`Stylesheet.cs:443-448,846-848`); not region-clipped, so the full width shows.
        let stroke_w = if curved {
            let pen_scale = if s <= 64.0 { 1.0 } else { 64.0 / s };
            let base = 0.16 * pen_scale;
            let wp = if taper && s >= 32.0 { base / 4.0 } else { base };
            wp.max(0.6 / s)
        } else {
            (0.10 * s).max(2.4) / s
        };
        let style = StrokeStyle::round(stroke_w);
        for group in &bc.groups {
            // A theme may force a single micro-border color (Atlas/FASA); otherwise
            // each group keeps its baked per-allegiance otu.css color. The geometry
            // is color-independent, so this needs no cache rebuild on a style switch.
            let color = micro_override.unwrap_or(&group.color);
            let fills: Vec<&Geometry> = group.fills.iter().map(Rc::as_ref).collect();
            // Fill the union at FILL_ALPHA (64/255) when filled. Hex: clip the
            // outline to that union so only the inner half shows (clean abutment).
            // Curved (FASA/Candy): stroke the spline directly, no clip
            // (RenderContext.cs:1856 else) — the smooth outline sits on the edge.
            c.draw_border_group(
                &fills,
                &group.stroke,
                m,
                if filled { Some(color) } else { None },
                0.25,
                color,
                &style,
                !curved,
            );
        }
    });
}
