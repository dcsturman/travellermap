//! Micro (per-sector) allegiance borders: filled interiors + hex-edge outlines,
//! grouped by polity across all loaded sectors and cached as world-space
//! `Path2d`s so a pan re-transforms instead of re-emitting tens of thousands of
//! path calls per frame.

use std::cell::RefCell;
use std::collections::HashMap;

use tmap_core::dto::SectorData;
use web_sys::Path2d;

use super::common::{
    allegiance_border_color, hex_neighbors, hex_sector, hex_vertex, hex_vertex_r, ViewState,
    HEX_VR,
};
use super::now;
use crate::canvas::Canvas2d;

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
                Some(geom) => !geom.groups.iter().any(|g| g.key == key && g.rset.contains(&hex)),
            }
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
        acc.into_values().collect()
    })
}

/// Allegiance borders: filled interior + hex-edge outline, drawn from cached
/// world-space `Path2d`s under a view transform (see `border_group_key` for the
/// cross-sector polity grouping). `dpr` composes into the world→device transform
/// so strokes stay crisp on retina.
pub(crate) fn draw_micro_borders(canvas: &Canvas2d, view: &ViewState, w: f64, h: f64, dpr: f64, sectors: &[&SectorData], filled: bool) {
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
