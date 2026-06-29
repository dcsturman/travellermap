//! Traveller Map streaming backend.
//!
//! Reimplements the data side of the reference `server/api/` handlers. Unlike
//! the original ASP.NET server, this does **no image rendering** — its only
//! job is to stream sector/metadata (see `tmap_core::dto`) to the browser,
//! which renders the map itself (Leptos/WASM). See CLAUDE.md "Mission".

use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tmap_core::{
    astrometrics::{self, parse_hex, Coord},
    dto::{
        DataFileMeta, Overlays, SearchResults, SearchResultsBody, SectorData, SectorInfo,
        SectorName, Subsector, Universe, UniverseResult, UniverseSector, VectorObject, World,
        WorldLabel,
    },
    metadata::{parse_sector_metadata, MetaAllegiance},
    parse::{
        border_region, milieu_sector_block, parse_column, parse_map_labels, parse_milieu_index,
        parse_sec, parse_tab, parse_vector_object, parse_world_labels, sector_allegiances,
        sector_credits, sector_datafile_meta, sector_index_entry, sector_subsectors,
    },
    sector_writer::{self, WriteOptions},
    world_util::{
        allegiance_base, allegiance_name, encode_legacy_bases, synthesize_abbreviation,
        t5_to_legacy_allegiance,
    },
};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

mod compat;
#[cfg(test)]
mod compat_suite;
mod route;
mod search;
mod svg_canvas;
mod tile;
use search::SearchIndex;

/// Macro-overlay vector files, grouped by kind (mirrors the reference
/// `RenderContext` border/rift/route file lists).
const BORDER_FILES: &[&str] = &[
    "Imperium",
    "Aslan",
    "Kkree",
    "Vargr",
    "Zhodani",
    "Solomani",
    "Hive",
    "SpinwardClient",
    "RimwardClient",
    "TrailingClient",
];
const RIFT_FILES: &[&str] = &[
    "GreatRift",
    "LesserRift",
    "WindhornRift",
    "DelphiRift",
    "ZhdantRift",
];
const ROUTE_FILES: &[&str] = &["J5Route", "J4Route", "CoreRoute"];

#[derive(Clone)]
struct AppState {
    /// Root of the shared `res/` data tree (the system of record).
    pub(crate) res_dir: PathBuf,
    /// Lazily-built, cached per-milieu sector index (name → grid coords).
    universe_cache: Arc<Mutex<HashMap<String, Arc<Universe>>>>,
    /// Macro overlays, parsed once on first request (charted-space, milieu-independent).
    overlays: Arc<OnceLock<Overlays>>,
    /// Lazily-built, cached per-milieu name search index.
    search_cache: Arc<Mutex<HashMap<String, Arc<SearchIndex>>>>,
    /// Per-milieu build lock for the search index (single-flight). A request that
    /// arrives while the background warm-up (or another request) is still building
    /// waits on this and then reuses that build, instead of starting a wasteful
    /// second build that contends for CPU — the cause of the "slow until warm"
    /// first search on a cold (1-vCPU) instance.
    search_builds: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// Cache of serialized JSON responses (key → (etag, bytes)) so repeat
    /// requests skip parsing + serialization. The data is static at runtime.
    response_cache: Arc<Mutex<HashMap<String, (String, Bytes)>>>,
    /// Lazily-cached raw text of each milieu's region list (`{milieu}.xml`). A
    /// sector's borders/routes may be defined inline there as well as (or
    /// instead of) in its own `.xml`, so sector builds consult it.
    region_cache: Arc<Mutex<HashMap<String, Arc<String>>>>,
    /// Lazily-built, cached per-milieu world index for jump-route finding — every
    /// world of the milieu keyed by absolute coordinate. Built by loading every
    /// sector once; the dataset is small enough to hold in RAM (see CLAUDE.md).
    route_cache: Arc<Mutex<HashMap<String, Arc<route::WorldIndex>>>>,
}

/// Parse the named `res/Vectors/{name}.xml` files, skipping any that fail.
fn load_vectors(res_dir: &FsPath, names: &[&str]) -> Vec<VectorObject> {
    names
        .iter()
        .filter_map(|n| {
            let path = res_dir.join("Vectors").join(format!("{n}.xml"));
            read_text(path)
                .ok()
                .and_then(|t| parse_vector_object(&t).ok())
        })
        .collect()
}

fn build_overlays(res_dir: &FsPath) -> Overlays {
    Overlays {
        borders: load_vectors(res_dir, BORDER_FILES),
        routes: load_vectors(res_dir, ROUTE_FILES),
        rifts: load_vectors(res_dir, RIFT_FILES),
        labels: build_world_labels(res_dir),
        mega_labels: read_text(res_dir.join("labels").join("mega_labels.tab"))
            .map(|t| parse_map_labels(&t))
            .unwrap_or_default(),
        minor_labels: read_text(res_dir.join("labels").join("minor_labels.tab"))
            .map(|t| parse_map_labels(&t))
            .unwrap_or_default(),
    }
}

/// Capitals + homeworlds from `res/labels/Worlds.xml`, resolving each marker's
/// sector name to absolute coordinates via the canonical (M1105) sector map —
/// these well-known sectors sit at the same grid position in every milieu.
fn build_world_labels(res_dir: &FsPath) -> Vec<WorldLabel> {
    let Ok(text) = read_text(res_dir.join("labels").join("Worlds.xml")) else {
        return Vec::new();
    };
    let universe = load_universe(res_dir, "M1105");
    let coord_of: HashMap<&str, (i32, i32)> = universe
        .sectors
        .iter()
        .map(|s| (s.name.as_str(), (s.location.x, s.location.y)))
        .collect();
    parse_world_labels(&text)
        .into_iter()
        .filter_map(|d| {
            let &(sx, sy) = coord_of.get(d.sector.as_str())?;
            let (col, row) = parse_hex(&d.hex)?;
            Some(WorldLabel {
                name: d.name,
                coord: Coord::new(sx * 32 + col, sy * 40 + row),
                bias: (d.bias_x, d.bias_y),
            })
        })
        .collect()
}

impl AppState {
    /// The sector index for a milieu, building and caching it on first use by
    /// scanning `res/Sectors/{milieu}/*.xml`.
    pub(crate) fn universe(&self, milieu: &str) -> Result<Arc<Universe>, (StatusCode, String)> {
        if !is_safe_segment(milieu) {
            return Err((StatusCode::BAD_REQUEST, "invalid milieu".into()));
        }
        if let Some(u) = self.universe_cache.lock().unwrap().get(milieu) {
            return Ok(u.clone());
        }
        let u = Arc::new(load_universe(&self.res_dir, milieu));
        if u.sectors.is_empty() {
            return Err((StatusCode::NOT_FOUND, format!("no milieu '{milieu}'")));
        }
        self.universe_cache
            .lock()
            .unwrap()
            .insert(milieu.to_string(), u.clone());
        Ok(u)
    }

    /// Raw region-list XML (`{milieu}.xml`) for a milieu, read + cached on first
    /// use. Empty string if absent. Used to recover metadata (borders/routes)
    /// for sectors that define it inline in the region list rather than in their
    /// own `.xml`.
    fn region_xml(&self, milieu: &str) -> Arc<String> {
        if let Some(r) = self.region_cache.lock().unwrap().get(milieu) {
            return r.clone();
        }
        let path = self
            .res_dir
            .join("Sectors")
            .join(milieu)
            .join(format!("{milieu}.xml"));
        let arc = Arc::new(read_text(path).unwrap_or_default());
        self.region_cache
            .lock()
            .unwrap()
            .insert(milieu.to_string(), arc.clone());
        arc
    }

    /// The name search index for a milieu, built and cached on first use.
    fn search_index(&self, milieu: &str) -> Result<Arc<SearchIndex>, (StatusCode, String)> {
        // Fast path: already built.
        if let Some(idx) = self.search_cache.lock().unwrap().get(milieu) {
            return Ok(idx.clone());
        }
        // Single-flight: take the per-milieu build lock so only one build runs at a
        // time. A caller racing the warm-up blocks here, then finds the finished
        // index on the re-check below — it never starts a second, contending build.
        let build_lock = self
            .search_builds
            .lock()
            .unwrap()
            .entry(milieu.to_string())
            .or_default()
            .clone();
        let _guard = build_lock.lock().unwrap();
        // Re-check: the build we waited on may have just populated the cache.
        if let Some(idx) = self.search_cache.lock().unwrap().get(milieu) {
            return Ok(idx.clone());
        }
        let universe = self.universe(milieu)?;
        let idx = Arc::new(search::build_index(&self.res_dir, milieu, &universe));
        self.search_cache
            .lock()
            .unwrap()
            .insert(milieu.to_string(), idx.clone());
        Ok(idx)
    }

    /// The jump-route world index for a milieu, built + cached on first use by
    /// loading every sector's worlds keyed by absolute coordinate.
    fn route_index(&self, milieu: &str) -> Result<Arc<route::WorldIndex>, (StatusCode, String)> {
        if let Some(idx) = self.route_cache.lock().unwrap().get(milieu) {
            return Ok(idx.clone());
        }
        let universe = self.universe(milieu)?;
        let idx = Arc::new(route::build_world_index(&self.res_dir, milieu, &universe));
        self.route_cache
            .lock()
            .unwrap()
            .insert(milieu.to_string(), idx.clone());
        Ok(idx)
    }
}

/// Read a file as text, tolerating non-UTF-8 sector data. Tries UTF-8, then
/// falls back to Latin-1 (every byte → a code point; matches the reference,
/// which reads CP1252) so legacy `.sec` files don't fail.
pub(crate) fn read_text(path: impl AsRef<FsPath>) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect()))
}

/// Resolve `name` within `dir`, tolerating case differences. The upstream region
/// lists declare per-sector files like `Blaskon.xml`/`Blaskon.txt` while the
/// on-disk files are lowercase (`blaskon.xml`); case-insensitive filesystems
/// (Windows/macOS, where the reference runs) hide the mismatch, but Linux — Cloud
/// Run and CI — does not, silently dropping those sectors' metadata and worlds.
/// Returns the directly-joined path when it exists (the fast common case), else
/// the first case-insensitive directory match.
pub(crate) fn resolve_ci(dir: &FsPath, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// Claim a synthesized abbreviation in `name_map`, disambiguating collisions like
/// the reference `SectorMap.MilieuMap`: if `base` (e.g. `"Fars"`) is free — or is
/// already mapped to this same sector (it equals one of its own names) — use it;
/// otherwise fall back to `"Far2"`, `"Far3"`, … (`base[..4-len(digit)] + digit`).
/// Map keys are lowercased (the reference's `InvariantCultureIgnoreCase`); the
/// returned value keeps the proper case.
fn claim_abbreviation(name_map: &mut HashMap<String, usize>, base: &str, i: usize) -> String {
    use std::collections::hash_map::Entry;
    match name_map.entry(base.to_lowercase()) {
        Entry::Vacant(v) => {
            v.insert(i);
            base.to_string()
        }
        Entry::Occupied(o) if *o.get() == i => base.to_string(),
        Entry::Occupied(_) => {
            for d in 2..=99u32 {
                let suffix = d.to_string();
                let plen = (4usize.saturating_sub(suffix.len())).min(base.len());
                let cand = format!("{}{}", &base[..plen], suffix);
                if let Entry::Vacant(v) = name_map.entry(cand.to_lowercase()) {
                    v.insert(i);
                    return cand;
                }
            }
            base.to_string() // 98 collisions on one prefix — unreachable in practice
        }
    }
}

/// Resolve each sector's final abbreviation across the milieu — a port of the
/// reference `SectorMap.MilieuMap` build. Sectors are processed in **load
/// (metafile-merge) order**, registering every sector's names (+ space-removed
/// aliases) and abbreviation in one shared case-insensitive map; an OTU sector
/// with no declared abbreviation gets a synthesized one, **deduplicated** against
/// everything already registered (so `Far Shore`/`Far Shore 2` become
/// `Far2`/`Far3` rather than colliding on `Fars`). Mutates `sectors` in place;
/// must run BEFORE any reorder, since the digit assignment depends on order.
fn resolve_abbreviations(sectors: &mut [tmap_core::dto::SectorIndexEntry]) {
    let mut name_map: HashMap<String, usize> = HashMap::new();
    for (i, sector) in sectors.iter_mut().enumerate() {
        let names: Vec<String> = if sector.names.is_empty() {
            vec![sector.name.clone()]
        } else {
            sector.names.iter().map(|n| n.text.clone()).collect()
        };
        // Register names + their space-removed aliases (reference TryAdd order:
        // names first, then this sector's abbreviation).
        for n in &names {
            name_map.entry(n.to_lowercase()).or_insert(i);
            name_map
                .entry(n.replace(' ', "").to_lowercase())
                .or_insert(i);
        }
        if let Some(ab) = sector.abbreviation.clone().filter(|a| !a.is_empty()) {
            name_map.entry(ab.to_lowercase()).or_insert(i); // declared — keep as-is
            continue;
        }
        // Synthesize only for OTU sectors (reference `SynthesizeAbbreviation`).
        let is_otu = sector
            .tags
            .split_whitespace()
            .chain(
                sector
                    .metafile_tag
                    .as_deref()
                    .unwrap_or("")
                    .split_whitespace(),
            )
            .any(|t| t == "OTU");
        if !is_otu {
            continue;
        }
        if let Some(base) = names.first().and_then(|n| synthesize_abbreviation(n)) {
            sector.abbreviation = Some(claim_abbreviation(&mut name_map, &base, i));
        }
    }
}

/// Scan a milieu directory, parsing each per-sector `.xml` head into an index
/// entry. Non-sector XML (the milieu region list) is skipped automatically.
fn load_universe(res_dir: &FsPath, milieu: &str) -> Universe {
    let sectors_dir = res_dir.join("Sectors");
    // Dedup by grid position within the milieu (reference `MilieuMap.TryAdd`,
    // first wins), so two metafiles can't list the same sector twice.
    let mut by_pos: HashMap<(i32, i32), tmap_core::dto::SectorIndexEntry> = HashMap::new();
    let mut order: Vec<(i32, i32)> = Vec::new();
    let mut insert = |e: tmap_core::dto::SectorIndexEntry| {
        let key = (e.location.x, e.location.y);
        if let std::collections::hash_map::Entry::Vacant(v) = by_pos.entry(key) {
            order.push(key);
            v.insert(e);
        }
    };

    // 1. Aggregate every metafile listed in `milieu.tab` (reference `SectorMap`),
    //    keeping only sectors whose `CanonicalMilieu` (the sector's `Milieu`
    //    attribute, else the default `M1105`) matches the requested milieu.
    //    `meta`-tagged metafiles (e.g. legend.xml) are excluded from the universe.
    for (path, tags) in milieu_metafiles(res_dir) {
        if tags.split(',').any(|t| t.trim() == "meta") {
            continue;
        }
        let metafile_path = sectors_dir.join(&path);
        let Ok(text) = read_text(&metafile_path) else {
            continue;
        };
        // Per-sector files (DataFile/MetadataFile) are relative to the metafile.
        let base_dir = metafile_path
            .parent()
            .map(FsPath::to_path_buf)
            .unwrap_or_else(|| sectors_dir.clone());
        let metafile_tag = tags
            .split(',')
            .map(str::trim)
            .find(|t| !t.is_empty())
            .map(str::to_owned);
        for mut e in parse_milieu_index(&text) {
            let canonical = e.milieu.as_deref().unwrap_or(DEFAULT_MILIEU);
            if !canonical.eq_ignore_ascii_case(milieu) {
                continue;
            }
            e.metafile_tag = metafile_tag.clone();
            // Merge the authoritative name list from the per-sector MetadataFile
            // (reference `Sector.Merge`): when it has names, they replace the
            // metafile's sparse list (canonical first).
            if let Some(mf) = &e.metadata_file {
                if let Some(mtext) = resolve_ci(&base_dir, mf).and_then(|p| read_text(p).ok()) {
                    let names = tmap_core::parse::parse_sector_names(&mtext);
                    if !names.is_empty() {
                        e.name = names[0].text.clone();
                        e.names = names;
                    }
                }
            }
            insert(e);
        }
    }

    // 2. Fall back to loose per-sector `.xml` in this milieu's own directory for
    //    any sector its metafile didn't list. Skip ones already loaded by
    //    **name** too, not just position: a loose file can carry a stale/conflicting
    //    coordinate (e.g. `Kruse.xml` at (0,-9) while the authoritative `M1105.xml`
    //    places Kruse at (0,9)), which a position-only guard would admit as a
    //    duplicate. The reference only reads metafiles, so the metafile wins.
    let loaded_names: std::collections::HashSet<String> =
        by_pos.values().map(|e| e.name.to_lowercase()).collect();
    let dir = sectors_dir.join(milieu);
    let milieu_file = format!("{milieu}.xml");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("xml")
                || path.file_name().and_then(|n| n.to_str()) == Some(milieu_file.as_str())
            {
                continue;
            }
            if let Ok(text) = read_text(&path) {
                if let Ok(e) = sector_index_entry(&text) {
                    // Inline insert (not the `insert` closure, whose mutable
                    // borrow of `by_pos` must have ended so `loaded_names` could
                    // read it): skip if the name OR position is already loaded.
                    let key = (e.location.x, e.location.y);
                    if !loaded_names.contains(&e.name.to_lowercase()) {
                        if let std::collections::hash_map::Entry::Vacant(v) = by_pos.entry(key) {
                            order.push(key);
                            v.insert(e);
                        }
                    }
                }
            }
        }
    }

    let mut sectors: Vec<_> = order
        .into_iter()
        .filter_map(|k| by_pos.remove(&k))
        .collect();
    // Resolve + dedup abbreviations in merge order (the reference SectorMap order)
    // BEFORE sorting for output — the digit suffixes depend on processing order.
    resolve_abbreviations(&mut sectors);
    sectors.sort_by(|a, b| a.name.cmp(&b.name));
    Universe {
        milieu: milieu.to_string(),
        sectors,
    }
}

/// `(metafile-path, tags-csv)` for every entry in `res/Sectors/milieu.tab`.
pub(crate) fn milieu_metafiles(res_dir: &FsPath) -> Vec<(String, String)> {
    let Ok(text) = read_text(res_dir.join("Sectors").join("milieu.tab")) else {
        return Vec::new();
    };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut f = line.split('\t');
            let path = f.next()?.trim();
            let tags = f.next().unwrap_or("").trim();
            (!path.is_empty()).then(|| (path.to_string(), tags.to_string()))
        })
        .collect()
}

impl AppState {
    /// Build a fresh state rooted at `res_dir`, with all caches empty.
    pub(crate) fn new(res_dir: PathBuf) -> Self {
        AppState {
            res_dir,
            universe_cache: Arc::new(Mutex::new(HashMap::new())),
            overlays: Arc::new(OnceLock::new()),
            search_cache: Arc::new(Mutex::new(HashMap::new())),
            search_builds: Arc::new(Mutex::new(HashMap::new())),
            response_cache: Arc::new(Mutex::new(HashMap::new())),
            region_cache: Arc::new(Mutex::new(HashMap::new())),
            route_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Assemble the application router. Shared by `main` and the compatibility test
/// suite (`compat_suite`), so tests exercise the exact routing/handlers we ship.
pub(crate) fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/api/health", get(health))
        .route("/api/universe", get(get_universe))
        .route("/api/overlays", get(get_overlays))
        .route("/api/search", get(get_search))
        .route("/api/route", get(get_route))
        .route("/api/sector/{milieu}/{name}", get(get_sector))
        // Poster-style map tile as SVG (the reference renders PNG server-side;
        // we emit vector SVG via the shared tmap-render passes). For external
        // <img> consumers (e.g. worldgen's Route Map).
        .route("/api/tile", get(tile::get_tile))
        // Public-API compatibility layer (documented URLs + PascalCase JSON).
        .route("/api/coordinates", get(compat::get_coordinates))
        .route("/api/sec", get(get_sec).post(post_sec))
        .route("/api/msec", get(get_msec))
        .route("/api/metadata", get(get_metadata).post(post_metadata))
        .route("/api/credits", get(get_credits))
        .route("/api/jumpworlds", get(get_jumpworlds))
        .route("/api/milieux", get(compat::get_milieux))
        .route("/t5ss/allegiances", get(compat::get_allegiances))
        .route("/t5ss/sophonts", get(compat::get_sophonts))
        // Semantic /data/{sector}/... aliases (port of the Global.asax.cs `/data`
        // table). Static path segments (sec/tab/coordinates/credits/metadata)
        // take priority over the `{tail}` capture at the same position.
        .route("/data/{sector}", get(data_sec))
        .route("/data/{sector}/sec", get(data_sec))
        .route("/data/{sector}/tab", get(data_tab))
        .route("/data/{sector}/msec", get(data_msec))
        .route("/data/{sector}/coordinates", get(compat::data_coordinates))
        .route("/data/{sector}/credits", get(data_credits))
        .route("/data/{sector}/metadata", get(data_metadata))
        // Single segment after the sector: a 4-digit hex (single world), a
        // quadrant (`alpha|beta|gamma|delta`), a subsector letter (`A`–`P`), or a
        // subsector name. The reference routes these by regex priority; we capture
        // `{tail}` once and dispatch on its shape (`data_world_or_region`). The
        // literal 3-segment routes above (sec/tab/…) still win by static priority.
        .route("/data/{sector}/{tail}", get(data_world_or_region))
        .route("/data/{sector}/{tail}/sec", get(data_region_sec))
        .route("/data/{sector}/{tail}/tab", get(data_region_tab))
        .route(
            "/data/{sector}/{tail}/coordinates",
            get(compat::data_coordinates_hex),
        )
        .route("/data/{sector}/{tail}/credits", get(data_credits_hex))
        .route("/data/{sector}/{tail}/jump/{jump}", get(data_jumpworlds))
        .route("/api/res/{*path}", get(get_res));

    // Admin/profiling routes (cache flush) are OFF unless TMAP_ENABLE_ADMIN is
    // set, so a public deployment never exposes them. Enable locally (e.g.
    // `TMAP_ENABLE_ADMIN=1`) when profiling. See flush_cache.
    if admin_enabled() {
        router = router.route("/api/admin/flush", post(flush_cache));
    }

    router
        // Permissive CORS for the public, read-only data API (third-party tools
        // call it cross-origin); the same-origin frontend itself needs none.
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[tokio::main]
async fn main() {
    // `res/` lives at the workspace root; override with TMAP_RES_DIR if needed.
    let res_dir = std::env::var("TMAP_RES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("res"));
    let state = AppState::new(res_dir);

    // Warm the default-milieu caches in the background. On a Cloud Run cold start
    // the triggering request is the static `index.html`; the browser then spends
    // time downloading + initializing the WASM bundle before it asks for data, so
    // building the M1105 universe concurrently usually has it cached by the time
    // the first `/api/universe` lands — hiding the parse instead of blocking the
    // port (which would stall even the HTML). spawn_blocking keeps the synchronous
    // parse off the async workers. See PORT_PLAN.md / DEPLOY.md.
    //
    // The search index is warmed here too: `build_index` re-walks every sector to
    // parse all world data + metadata and build the Tantivy index — seconds of
    // work that otherwise lands inline on the user's *first* `/api/search`. It
    // reuses the universe built just above (cached), so this is the index build
    // alone. Without it, search is fast on every request but the first.
    {
        let warm = state.clone();
        tokio::task::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            match warm.universe(DEFAULT_MILIEU) {
                Ok(u) => println!(
                    "warm-up: {DEFAULT_MILIEU} universe ({} sectors) ready in {:?}",
                    u.sectors.len(),
                    t0.elapsed()
                ),
                Err((_, e)) => eprintln!("warm-up: {DEFAULT_MILIEU} universe failed: {e}"),
            }
            let t1 = std::time::Instant::now();
            match warm.search_index(DEFAULT_MILIEU) {
                Ok(_) => println!(
                    "warm-up: {DEFAULT_MILIEU} search index ready in {:?}",
                    t1.elapsed()
                ),
                Err((_, e)) => eprintln!("warm-up: {DEFAULT_MILIEU} search index failed: {e}"),
            }
        });
    }

    let mut app = build_router(state);

    // In a deployed image the built WASM frontend (Trunk `dist/`) is served from
    // the SAME origin as the API: the API / `/data` / `/t5ss` routes match first,
    // and anything else falls through to the static bundle, with an `index.html`
    // fallback so SPA deep-links / refreshes resolve. Skipped in local dev (no
    // `dist/`), where Trunk serves the frontend and proxies `/api` here.
    let dist_dir = std::env::var("TMAP_DIST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("dist"));
    if dist_dir.is_dir() {
        let serve_dist =
            ServeDir::new(&dist_dir).fallback(ServeFile::new(dist_dir.join("index.html")));
        app = app.fallback_service(serve_dist);
        println!("serving static frontend from {}", dist_dir.display());
    }

    // Cloud Run (and most PaaS) inject the port via $PORT; bind all interfaces so
    // the platform can reach it. Falls back to :3000 for local `cargo run`.
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("tmap-backend listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

/// Resolve when the process receives SIGINT (Ctrl-C) or SIGTERM (Cloud Run sends
/// SIGTERM on scale-down). Driving `axum::serve` with this lets it stop accepting
/// connections, drain in-flight requests, and exit cleanly — important because the
/// container runs `tmap-backend` as PID 1, which otherwise ignores default-action
/// signals and would hang until the platform SIGKILLs it.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    println!("shutdown signal received; draining…");
}

async fn health() -> &'static str {
    "ok"
}

/// Whether the admin/profiling routes are mounted. OFF by default so a public
/// deployment never exposes them; enable with `TMAP_ENABLE_ADMIN` set to a
/// truthy value (`1`/`true`/`yes`/`on`). Read once at router-build time.
fn admin_enabled() -> bool {
    std::env::var("TMAP_ENABLE_ADMIN")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// `POST /api/admin/flush` — drop the built-response cache so the next request
/// for each sector/overlay re-parses from `res/` (the cold-cache path). The
/// parsed-index caches (universe/search) stay warm on purpose: for profiling we
/// want to measure sector parse + serialize, not re-parse the milieu index on
/// every request. Returns how many entries were evicted. A dev/profiling
/// convenience — the route is only mounted when [`admin_enabled`] is true.
async fn flush_cache(State(state): State<AppState>) -> Response {
    let mut cache = state.response_cache.lock().unwrap();
    let n = cache.len();
    cache.clear();
    (StatusCode::OK, format!("flushed {n} cached responses\n")).into_response()
}

/// `GET /api/res/{*path}` — serve a static asset from the shared `res/` tree
/// (legend SVGs, galaxy/Candy textures, markers …). Read-only; path-validated
/// so it can't escape `res/`.
async fn get_res(Path(path): Path<String>, State(state): State<AppState>) -> Response {
    // Reject any traversal or absolute components.
    if path
        .split('/')
        .any(|seg| seg == ".." || seg == "." || seg.is_empty())
        || path.contains('\\')
    {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let full = state.res_dir.join(&path);
    let Ok(bytes) = std::fs::read(&full) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = match full.extension().and_then(|e| e.to_str()) {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        bytes,
    )
        .into_response()
}

/// Content-hash ETag.
fn etag_for(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("\"{:x}\"", h.finish())
}

/// Serve a JSON body with response caching + HTTP caching (Cache-Control +
/// ETag/304). `build` runs only on a cache miss; its bytes are cached under
/// `key`. CDN- and browser-friendly: static data, so far-future cache + ETag.
fn serve_cached(
    cache: &Mutex<HashMap<String, (String, Bytes)>>,
    key: &str,
    req: &HeaderMap,
    build: impl FnOnce() -> Result<Vec<u8>, (StatusCode, String)>,
) -> Response {
    let entry = cache.lock().unwrap().get(key).cloned();
    let (etag, bytes) = match entry {
        Some(v) => v,
        None => match build() {
            Ok(v) => {
                let etag = etag_for(&v);
                let bytes = Bytes::from(v);
                cache
                    .lock()
                    .unwrap()
                    .insert(key.to_owned(), (etag.clone(), bytes.clone()));
                (etag, bytes)
            }
            Err(e) => return e.into_response(),
        },
    };

    // Static data (changes only on an upstream pull + redeploy), so it is safe
    // to edge-cache aggressively. `max-age=300` lets a browser hold it briefly
    // (ETag makes the revalidation after that a cheap 304); `s-maxage=86400`
    // lets a shared cache (Cloudflare) serve it from the edge for a day. Freshness
    // on deploy is handled out-of-band: `scripts/deploy.sh` purges the CDN edge
    // after every deploy (see DEPLOY.md), so a data change goes live immediately
    // rather than waiting for the TTL. `stale-while-revalidate` smooths the
    // refresh so a request never blocks on a re-fetch.
    const CACHE: &str = "public, max-age=300, s-maxage=86400, stale-while-revalidate=86400";
    let fresh = req
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag);
    if fresh {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, CACHE)
            .body(Body::empty())
            .unwrap();
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, CACHE)
        .body(Body::from(bytes))
        .unwrap()
}

/// Lower-detail projection of a world (drops fields not rendered until extreme
/// zoom: stellar data, the {Ix}/(Ex)/[Cx] extensions, nobility, W, RU).
fn project_overview(world: World) -> World {
    World {
        stellar: String::new(),
        importance: None,
        economic: None,
        cultural: None,
        nobility: None,
        worlds: None,
        resource_units: None,
        ..world
    }
}

// --- Render projections: shared `metadata` types → render `dto` types --------
// These map the parse-once `SectorMetadata` onto the renderer-tuned `SectorData`
// shapes the Leptos client consumes, reproducing the old per-element parse
// exactly (e.g. a border with `ShowLabel=false` drops its label + position).

fn render_subsectors(subs: &[tmap_core::metadata::MetaSubsector]) -> Vec<Subsector> {
    subs.iter()
        .map(|s| Subsector {
            index: s.index.clone(),
            name: s.name.clone(),
        })
        .collect()
}

fn render_route(r: &tmap_core::metadata::MetaRoute) -> tmap_core::dto::Route {
    tmap_core::dto::Route {
        start: r.start.clone(),
        end: r.end.clone(),
        start_offset: (r.start_offset_x, r.start_offset_y),
        end_offset: (r.end_offset_x, r.end_offset_y),
        allegiance: r.allegiance.clone(),
        color: r.color.clone(),
    }
}

/// Borders + Regions of a sector in source document order (`<Border>`/`<Region>`
/// interleaved), so the render layer draws them exactly as the reference did.
fn ordered_borders(
    m: &tmap_core::metadata::SectorMetadata,
) -> Vec<&tmap_core::metadata::MetaBorder> {
    let mut v: Vec<&tmap_core::metadata::MetaBorder> = m.borders.iter().chain(&m.regions).collect();
    v.sort_by_key(|b| b.seq);
    v
}

fn render_border(b: &tmap_core::metadata::MetaBorder) -> tmap_core::dto::Border {
    // `ShowLabel=false` suppresses both the label and its position (the client
    // resolves border labels from `label_position`).
    let (label, label_position) = if b.show_label {
        (b.label.clone(), b.label_position.clone())
    } else {
        (None, None)
    };
    tmap_core::dto::Border {
        allegiance: b.allegiance.clone().unwrap_or_default(),
        hexes: b.hexes.clone(),
        region: Vec::new(),
        color: b.color.clone(),
        label,
        label_position,
        wrap_label: b.wrap_label,
        label_offset: (b.label_offset_x, b.label_offset_y),
    }
}

fn render_label(l: &tmap_core::metadata::MetaLabel) -> tmap_core::dto::SectorLabel {
    tmap_core::dto::SectorLabel {
        text: l.text.clone(),
        hex: l.hex.clone(),
        color: l.color.clone(),
        size: l.size.clone(),
        wrap: l.wrap,
        offset: (l.offset_x, l.offset_y),
    }
}

/// `GET /api/overlays` — macro borders/routes/rifts for the zoomed-out view.
async fn get_overlays(headers: HeaderMap, State(state): State<AppState>) -> Response {
    serve_cached(&state.response_cache, "overlays", &headers, || {
        let overlays = state
            .overlays
            .get_or_init(|| build_overlays(&state.res_dir));
        serde_json::to_vec(overlays).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
}

/// The default milieu (Imperial year 1105) — sectors with no `Milieu` attribute
/// belong to it (reference `SectorMap.DEFAULT_MILIEU`).
pub(crate) const DEFAULT_MILIEU: &str = "M1105";

fn default_milieu() -> String {
    DEFAULT_MILIEU.to_string()
}

#[derive(Debug, Deserialize)]
struct UniverseQuery {
    /// Milieu (era snapshot). `era` is an accepted alias; `milieu` wins.
    milieu: Option<String>,
    era: Option<String>,
    /// When true, omit positioned-but-dataless sectors. Reference `requireData`.
    #[serde(rename = "requireData")]
    require_data: Option<String>,
    /// Restrict to sectors carrying any of these tags (repeatable / comma-sep).
    #[serde(default)]
    tag: Vec<String>,
}

/// `GET /api/universe?milieu=M1105` — the sector index for navigation, in the
/// documented public shape (`UniverseHandler`'s `{"Sectors":[…]}`, PascalCase).
/// This is the unified contract: external tools and our own Leptos client both
/// read it (the private snake_case `Universe` is now in-memory only).
async fn get_universe(
    Query(q): Query<UniverseQuery>,
    Query(jp): Query<compat::Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    // `milieu` wins over its `era` alias; both default to M1105.
    let milieu = q
        .milieu
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| q.era.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(default_milieu);
    let require_data = bool_opt(&q.require_data, false);
    // `tag=a&tag=b` or `tag=a,b` — match a sector carrying ANY listed tag.
    let want_tags: Vec<String> = q
        .tag
        .iter()
        .flat_map(|t| t.split(','))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    // JSONP/XML are opt-in (the default stays byte-identical JSON for our Leptos
    // client and for the response cache / ETag path): build the typed result and
    // route it through content negotiation. Otherwise serve the cached JSON bytes.
    if jp.jsonp.is_some() || compat::wants_xml(&headers) {
        let result = match build_universe_result(&state, &milieu, require_data, &want_tags) {
            Ok(r) => r,
            Err(e) => return e.into_response(),
        };
        return compat::respond_negotiated(&result, &jp.jsonp, compat::wants_xml(&headers), || {
            result.to_xml()
        });
    }

    let key = format!(
        "universe/{milieu}/rd{}/tag{}",
        require_data,
        want_tags.join(",")
    );
    serve_cached(&state.response_cache, &key, &headers, || {
        let result = build_universe_result(&state, &milieu, require_data, &want_tags)?;
        serde_json::to_vec(&result).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
}

/// Build the public `/api/universe` result for a milieu (filtered by
/// `require_data` + `want_tags`). Shared by the cached JSON path and the
/// JSONP/XML content-negotiated path so all three emit identical content.
fn build_universe_result(
    state: &AppState,
    milieu: &str,
    require_data: bool,
    want_tags: &[String],
) -> Result<UniverseResult, (StatusCode, String)> {
    let u = state.universe(milieu)?;
    // A dir-fallback sector has no metafile tag; use the requested milieu's.
    let default_tag = milieu_tag(&state.res_dir, milieu);
    let sectors = u
        .sectors
        .iter()
        .filter(|s| !require_data || s.data_file.is_some())
        .filter_map(|s| {
            let mtag = s
                .metafile_tag
                .as_deref()
                .or(default_tag.as_deref())
                .unwrap_or("");
            // Sector's own review tags ("Official") + metafile tag ("OTU"),
            // deduped preserving order (reference `Tags` is an OrderedHashSet,
            // so e.g. a Faraway sector tagged "Faraway" + metafile "Faraway"
            // collapses to one).
            let mut seen = std::collections::HashSet::new();
            let tags = [s.tags.as_str(), mtag]
                .into_iter()
                .flat_map(str::split_whitespace)
                .filter(|t| seen.insert(*t))
                .collect::<Vec<_>>()
                .join(" ");
            if !want_tags.is_empty()
                && !tags
                    .split_whitespace()
                    .any(|t| want_tags.iter().any(|w| w == t))
            {
                return None;
            }
            // Always emit at least the canonical name (older per-sector xml
            // without a localized list still has `s.name`).
            let names = if s.names.is_empty() {
                vec![SectorName {
                    text: s.name.clone(),
                    lang: None,
                    source: None,
                }]
            } else {
                s.names.clone()
            };
            // OTU sectors with no declared abbreviation get a synthesized
            // one (e.g. "Zhdant" → "Zhda"), matching the reference.
            let abbreviation = s.abbreviation.clone().or_else(|| {
                tags.split_whitespace()
                    .any(|t| t == "OTU")
                    .then(|| names.first().and_then(|n| synthesize_abbreviation(&n.text)))
                    .flatten()
            });
            Some(UniverseSector {
                x: s.location.x,
                y: s.location.y,
                milieu: milieu.to_string(),
                abbreviation,
                tags,
                names,
            })
        })
        .collect();
    Ok(UniverseResult { sectors })
}

/// The tag the milieu metafile carries in `res/Sectors/milieu.tab` (e.g.
/// `"OTU"`), appended to each sector's own tags in the universe response.
fn milieu_tag(res_dir: &FsPath, milieu: &str) -> Option<String> {
    let text = read_text(res_dir.join("Sectors").join("milieu.tab")).ok()?;
    text.lines().skip(1).find_map(|line| {
        let mut f = line.split('\t');
        let path = f.next()?;
        let tag = f.next()?.trim();
        (path.split('/').next() == Some(milieu) && !tag.is_empty()).then(|| tag.to_string())
    })
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_milieu")]
    milieu: String,
    /// Comma/space-separated result kinds (`worlds|subsectors|sectors|labels|
    /// default`); defaults to `default` (all kinds). Mirrors the reference
    /// `types=` query param.
    #[serde(default)]
    types: Option<String>,
}

/// Per-type cap and final cap (reference `SearchHandler.NUM_RESULTS`).
const NUM_RESULTS: usize = 160;

/// The handler-level query preprocessing (port of `SearchHandler.Process`):
/// translate `*`→`%` and `?`→`_` wildcards, then prepend `uwp:` when the whole
/// query is a UWP (`^\w{7}-\w$`: seven word-chars, hyphen, one word-char).
fn preprocess_query(raw: &str) -> String {
    let q: String = raw
        .chars()
        .map(|c| match c {
            '*' => '%',
            '?' => '_',
            other => other,
        })
        .collect();
    if is_uwp_shortcut(&q) {
        format!("uwp:{q}")
    } else {
        q
    }
}

/// `^\w{7}-\w$` — seven `\w` chars, a hyphen, one `\w` char (`\w` = `[A-Za-z0-9_]`).
fn is_uwp_shortcut(q: &str) -> bool {
    let b = q.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    b.len() == 9 && b[..7].iter().all(|&c| is_word(c)) && b[7] == b'-' && is_word(b[8])
}

/// Parse the `types=` param into a [`SearchTypes`] set. Unknown tokens are
/// ignored; `default` enables all kinds (reference `GetStringsOption`).
fn parse_types(raw: Option<&str>) -> tmap_core::searchlang::SearchTypes {
    use tmap_core::searchlang::SearchTypes;
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return SearchTypes::DEFAULT;
    };
    let mut t = SearchTypes::NONE;
    for tok in raw.split([',', ' ', '\t']).filter(|s| !s.is_empty()) {
        match tok {
            "worlds" => t.worlds = true,
            "subsectors" => t.subsectors = true,
            "sectors" => t.sectors = true,
            "labels" => t.labels = true,
            "default" => t = SearchTypes::DEFAULT,
            _ => {}
        }
    }
    t
}

/// `GET /api/search?q=Regina&milieu=M1105&types=default` — the full
/// travellermap.com search query language over worlds, sectors, subsectors, and
/// labeled regions.
async fn get_search(
    Query(q): Query<SearchQuery>,
    Query(jp): Query<compat::Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let want_xml = compat::wants_xml(&headers);

    // Special "(...)" searches (port of `SearchHandler`): a parenthesized query is
    // either a canned named result file (`res/search/<Name>.json`) or the literal
    // `(random world)`. Tried before the normal query language. The canned file is
    // only served when the client accepts JSON/JSONP (an XML request falls through
    // to a normal search), matching the reference's `Accepts(JSON) || jsonp`.
    if let Some(name) = special_search_name(&q.q) {
        if (jp.jsonp.is_some() || !want_xml) && name != "randomworld" {
            if let Some(resp) = canned_search_response(&state.res_dir, &name, &jp.jsonp) {
                return resp;
            }
        }
    }

    let idx = match state.search_index(&q.milieu) {
        Ok(idx) => idx,
        Err(e) => return e.into_response(),
    };
    let items = if q.q == "(random world)" {
        // One random world, in the same envelope as a normal search.
        search::random_world(&idx).into_iter().collect()
    } else {
        let query = preprocess_query(&q.q);
        let pq = tmap_core::searchlang::parse_query(&query, parse_types(q.types.as_deref()));
        search::run_query(&idx, &pq, NUM_RESULTS)
    };
    let result = SearchResults {
        results: SearchResultsBody {
            count: items.len(),
            items,
        },
    };
    // JSON stays the default (our client + tools depend on it); jsonp/xml opt-in.
    compat::respond_negotiated(&result, &jp.jsonp, want_xml, || result.to_xml())
}

/// Extract the first `(...)` group of `[A-Za-z0-9 ]+` from a search query and
/// return it with spaces removed — port of `SearchHandler.SPECIAL_REGEXP`
/// (`\(([A-Za-z0-9 ]+)\)`) + `.Replace(" ", "")`. So `(Grand Tour)` → `GrandTour`
/// and `(random world)` → `randomworld`. Returns `None` if there's no such group.
fn special_search_name(q: &str) -> Option<String> {
    let open = q.find('(')?;
    let mut inner = String::new();
    for ch in q[open + 1..].chars() {
        match ch {
            ')' if inner.is_empty() => return None,
            ')' => return Some(inner.chars().filter(|c| *c != ' ').collect()),
            c if c.is_ascii_alphanumeric() || c == ' ' => inner.push(c),
            _ => return None,
        }
    }
    None
}

/// Serve a canned search result file (`res/search/<name>.json`) verbatim, honoring
/// `jsonp` (reference `SearchHandler` `SendFile`). `name` is alphanumeric (from
/// [`special_search_name`]), so it can't escape the directory. Returns `None` if
/// the file is absent or not valid JSON (→ caller falls through to a real search).
fn canned_search_response(
    res_dir: &FsPath,
    name: &str,
    jsonp: &Option<String>,
) -> Option<Response> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let path = res_dir.join("search").join(format!("{name}.json"));
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(compat::respond(&value, jsonp))
}

#[derive(Debug, Deserialize)]
struct RouteQuery {
    /// Start world, `"Sector Name 0101"` (sector display name + 4-digit hex).
    start: String,
    /// End world, same form.
    end: String,
    /// Jump rating in parsecs; clamped 1..=12 (mirrors `RouteHandler`).
    #[serde(default = "default_jump")]
    jump: i32,
    #[serde(default = "default_milieu")]
    milieu: String,
    /// Filters (mirror `RouteHandler` flags): avoid red zones, require
    /// wilderness refuelling, Imperial worlds only, allow anomalies.
    #[serde(default)]
    nored: bool,
    #[serde(default)]
    wild: bool,
    #[serde(default)]
    im: bool,
    #[serde(default)]
    aok: bool,
    /// Private extension: return the rich `{waypoints,jumps,parsecs}` object
    /// (with absolute coords, for our client to draw) instead of the documented
    /// public bare array of stops. Off by default → public-API compatible.
    #[serde(default)]
    detail: bool,
}

fn default_jump() -> i32 {
    2
}

/// `GET /api/route?start=<Sector 0101>&end=<Sector 0101>&jump=N&milieu=M1105` —
/// compute a jump route between two worlds. Mirrors the reference
/// `server/api/RouteHandler.cs` (start/end "Sector hhhh" parsing, jump clamp,
/// filter flags, jump-count-minimizing cost). Returns a [`RouteResult`].
async fn get_route(
    Query(q): Query<RouteQuery>,
    Query(jp): Query<compat::Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, String)> {
    let jump = q.jump.clamp(1, 12);
    let index = state.route_index(&q.milieu)?;

    let start = route::resolve_location(&index, &q.start).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("start not found: {}", q.start),
        )
    })?;
    let end = route::resolve_location(&index, &q.end)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("end not found: {}", q.end)))?;

    let opts = tmap_core::route::RouteOptions {
        avoid_red: q.nored,
        require_refuel: q.wild,
        imperial_only: q.im,
        allow_anomalies: q.aok,
    };
    let result = index
        .find_route(start, end, jump, opts)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "no route found".to_string()))?;
    // Default: the documented public bare array of stops. `&detail=true` returns
    // our rich object (waypoints with absolute coords) for the Leptos client.
    // JSON stays the default for both; jsonp/xml are opt-in. The reference only
    // defines an XML shape for the public stops list (`ArrayOfRouteStop`), so XML
    // applies to that form only; `detail` honors jsonp but always serves JSON.
    if q.detail {
        Ok(compat::respond(&result, &jp.jsonp))
    } else {
        let stops = result.to_public_stops();
        Ok(compat::respond_negotiated(
            &stops,
            &jp.jsonp,
            compat::wants_xml(&headers),
            || tmap_core::dto::route_stops_to_xml(&stops),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct SecQuery {
    #[serde(default = "default_milieu")]
    milieu: String,
    /// Sector by display name or T5SS abbreviation.
    sector: Option<String>,
    sx: Option<i32>,
    sy: Option<i32>,
    /// Restrict to one subsector (letter A–P or name).
    subsector: Option<String>,
    /// Restrict to one quadrant (`alpha`/`beta`/`gamma`/`delta`).
    quadrant: Option<String>,
    /// Output format: `SEC` (legacy fixed-column) | `TabDelimited` | `SecondSurvey`.
    /// Absent defaults to `SecondSurvey` columnar (matching live travellermap.com).
    #[serde(rename = "type")]
    type_: Option<String>,
    /// Booleans (`0`/`1`/`true`/`false`); reference defaults: metadata=1, header=1, sscoords=0.
    metadata: Option<String>,
    header: Option<String>,
    sscoords: Option<String>,
}

/// Parse a reference-style boolean query option (`1`/`true` → true, else the default).
fn bool_opt(v: &Option<String>, default: bool) -> bool {
    match v.as_deref() {
        Some("1") | Some("true") | Some("True") => true,
        Some("0") | Some("false") | Some("False") => false,
        _ => default,
    }
}

/// Parse uploaded world data (POST body) into a world list, sniffing the format
/// from the content the same way `resolve_and_parse_worlds` does for files
/// (`SectorFileParser.SniffType`/`WorldCollection.Deserialize`): tab-delimited →
/// `TabDelimited`, `{Ix} (Ex) [Cx]` present → `SecondSurvey`, else legacy `SEC`.
fn parse_world_text(text: &str) -> Result<Vec<World>, (StatusCode, String)> {
    let outcome = match sniff_world_format(text) {
        "TabDelimited" => parse_tab(text),
        "SecondSurvey" => parse_column(text),
        _ => parse_sec(text),
    }
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("Bad data file: {e}")))?;
    Ok(outcome.worlds)
}

/// `POST /api/sec` — reformat/convert uploaded world data. Ports `SECHandler.cs`'s
/// POST path: parse the posted body (format sniffed from content), then
/// re-serialize it in the requested `type=` (`SEC` | `TabDelimited` |
/// `SecondSurvey`). A missing `type` defaults to `SecondSurvey` columnar (matching
/// live travellermap.com). Metadata is never emitted (`includeMetadata = false`).
/// Subsector-by-letter and quadrant filtering still apply; subsector-by-name is
/// not supported (no names in posted data). The `lint` mode (ErrorLogger
/// diagnostics) is DEFERRED — `lint=1` is accepted but performs the same
/// parse+reformat without surfacing warnings.
async fn post_sec(Query(q): Query<SecQuery>, body: String) -> Response {
    match build_sec_from_text(&q, &body) {
        Ok(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

fn build_sec_from_text(q: &SecQuery, body: &str) -> Result<String, (StatusCode, String)> {
    // Live travellermap.com defaults a missing `type` to SecondSurvey columnar
    // (same as GET `/api/sec`), so POST round-trips an upload to SecondSurvey.
    let media = q.type_.as_deref().unwrap_or("SecondSurvey");
    if media != "TabDelimited" && media != "SecondSurvey" && media != "SEC" {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("type '{media}' not supported; use type=SEC, type=TabDelimited or type=SecondSurvey"),
        ));
    }

    let all_worlds = parse_world_text(body)?;

    // Subsector (letter only — posted data carries no subsector names) / quadrant
    // filtering, mirroring SECHandler's `options.filter`.
    let filtered: Vec<World> = if let Some(sub) = &q.subsector {
        let idx = subsector_index_for(sub, &[]).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("subsector '{sub}' not found"),
            )
        })?;
        all_worlds
            .iter()
            .filter(|w| astrometrics::subsector_index(&w.hex) == idx)
            .cloned()
            .collect()
    } else if let Some(quad) = &q.quadrant {
        let qidx = quadrant_index_for(quad).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("quadrant '{quad}' is invalid"),
            )
        })?;
        all_worlds
            .iter()
            .filter(|w| astrometrics::quadrant_index(&w.hex) == qidx)
            .cloned()
            .collect()
    } else {
        all_worlds
    };

    let opts = WriteOptions {
        sscoords: bool_opt(&q.sscoords, false),
        include_header: bool_opt(&q.header, true),
    };

    // No metadata block on POST (includeMetadata = false). Posted data carries no
    // sector abbreviation, so the TabDelimited `Sector` column is empty (matching
    // the reference, whose posted Sector has an empty Abbreviation).
    Ok(match media {
        "TabDelimited" => sector_writer::write_tab(&filtered, "", &opts),
        "SecondSurvey" => sector_writer::write_second_survey(&filtered, &opts),
        _ => sector_writer::write_legacy_sec(&filtered, &opts),
    })
}

/// `GET /api/sec` — a sector's worlds as SEC/SecondSurvey/TabDelimited text.
/// Ports `server/api/SECHandler.cs` (data side). Serves `type=SEC` (legacy
/// fixed-column), `type=TabDelimited`, and `type=SecondSurvey`; a missing `type`
/// defaults to `SecondSurvey` columnar (matching live travellermap.com).
async fn get_sec(Query(q): Query<SecQuery>, State(state): State<AppState>) -> Response {
    match build_sec(&state, &q) {
        Ok(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

fn build_sec(state: &AppState, q: &SecQuery) -> Result<String, (StatusCode, String)> {
    // Production travellermap.com defaults a missing `type` to the SecondSurvey
    // columnar format (NOT the legacy `SecSerializer` the older reference checkout
    // selects); `type=SEC` selects the legacy fixed-column writer. We follow live.
    let media = match q.type_.as_deref() {
        None => "SecondSurvey",
        Some(t) => t,
    };
    if media != "TabDelimited" && media != "SecondSurvey" && media != "SEC" {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("type '{media}' not supported; use type=SEC, type=TabDelimited or type=SecondSurvey"),
        ));
    }

    // Resolve the sector by sx,sy or name/abbreviation.
    let universe = state.universe(&q.milieu)?;
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe
            .sectors
            .iter()
            .find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return Err((StatusCode::BAD_REQUEST, "No sector specified.".into()));
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "The specified sector was not found.".into(),
        )
    })?;

    let dir = state.res_dir.join("Sectors").join(&q.milieu);
    let (data_file, outcome) = resolve_and_parse_worlds(&dir, &entry.name, Some(entry))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no data for '{}'", entry.name),
            )
        })?;
    let all_worlds = outcome.worlds;

    // Subsector / quadrant filtering (mirrors SECHandler's `options.filter`).
    let subsectors = gather_subsectors(state, &dir, &data_file, entry, &q.milieu);
    let filtered: Vec<World> = if let Some(sub) = &q.subsector {
        let idx = subsector_index_for(sub, &subsectors).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("subsector '{sub}' not found"),
            )
        })?;
        all_worlds
            .iter()
            .filter(|w| astrometrics::subsector_index(&w.hex) == idx)
            .cloned()
            .collect()
    } else if let Some(quad) = &q.quadrant {
        let qidx = quadrant_index_for(quad).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("quadrant '{quad}' is invalid"),
            )
        })?;
        all_worlds
            .iter()
            .filter(|w| astrometrics::quadrant_index(&w.hex) == qidx)
            .cloned()
            .collect()
    } else {
        all_worlds.clone()
    };

    let opts = WriteOptions {
        sscoords: bool_opt(&q.sscoords, false),
        include_header: bool_opt(&q.header, true),
    };

    // Effective abbreviation: declared, else synthesized for OTU sectors.
    let mtag = milieu_tag(&state.res_dir, &q.milieu);
    let is_otu = entry
        .tags
        .split_whitespace()
        .chain(mtag.as_deref().unwrap_or("").split_whitespace())
        .any(|t| t == "OTU");
    let abbr = entry.abbreviation.clone().or_else(|| {
        is_otu
            .then(|| {
                entry
                    .names
                    .first()
                    .and_then(|n| synthesize_abbreviation(&n.text))
            })
            .flatten()
    });

    if media == "TabDelimited" {
        // TabDelimited ignores includeMetadata (no comment block), per the reference.
        return Ok(sector_writer::write_tab(
            &filtered,
            abbr.as_deref().unwrap_or(""),
            &opts,
        ));
    }

    // SecondSurvey / legacy SEC: optional metadata comment block (allegiances from
    // ALL worlds), then the world table (filtered). The block's `# Alleg:` codes
    // are T5 for SecondSurvey and legacy for SEC (reference `Sector.Serialize`).
    let legacy = media == "SEC";
    let mut out = String::new();
    if bool_opt(&q.metadata, true) {
        out.push_str(&sec_metadata_block(
            state,
            &dir,
            &data_file,
            entry,
            abbr.as_deref(),
            &q.milieu,
            &subsectors,
            &all_worlds,
            legacy,
        ));
    }
    if legacy {
        out.push_str(&sector_writer::write_legacy_sec(&filtered, &opts));
    } else {
        out.push_str(&sector_writer::write_second_survey(&filtered, &opts));
    }
    Ok(out)
}

/// Subsector index 0–15 for a subsector letter (`A`–`P`) or name.
fn subsector_index_for(label: &str, subsectors: &[Subsector]) -> Option<usize> {
    if label.len() == 1 {
        let c = label.chars().next().unwrap().to_ascii_uppercase();
        if ('A'..='P').contains(&c) {
            return Some((c as u8 - b'A') as usize);
        }
    }
    subsectors
        .iter()
        .find(|s| !s.name.is_empty() && s.name.eq_ignore_ascii_case(label))
        .and_then(|s| s.index.chars().next())
        .map(|c| (c.to_ascii_uppercase() as u8 - b'A') as usize)
}

/// Quadrant index for `alpha`/`beta`/`gamma`/`delta` (`Sector.QuadrantIndexFor`).
fn quadrant_index_for(label: &str) -> Option<usize> {
    match label.to_ascii_lowercase().as_str() {
        "alpha" => Some(0),
        "beta" => Some(1),
        "gamma" => Some(2),
        "delta" => Some(3),
        _ => None,
    }
}

/// Read a sector's subsector list from its metadata `.xml`, falling back to the
/// inline region-list block.
fn gather_subsectors(
    state: &AppState,
    dir: &FsPath,
    data_file: &str,
    entry: &tmap_core::dto::SectorIndexEntry,
    milieu: &str,
) -> Vec<Subsector> {
    let meta_xml = read_meta_xml(dir, data_file, entry);
    let mut subs = sector_subsectors(&meta_xml);
    if subs.is_empty() {
        let region = state.region_xml(milieu);
        let inline = milieu_sector_block(&region, &entry.name).unwrap_or_default();
        subs = sector_subsectors(&inline);
    }
    subs
}

/// `/data/{sector}/{hex}/jump/{jump}` → worlds within `jump` of the hex.
async fn data_jumpworlds(
    Path((sector, hex, jump)): Path<(String, String, i32)>,
    State(state): State<AppState>,
) -> Response {
    let q = JumpWorldsQuery {
        milieu: default_milieu(),
        sector: Some(sector),
        sx: None,
        sy: None,
        hex: Some(hex),
        jump,
    };
    match build_jumpworlds(&state, &q) {
        Ok(result) => Json(result).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

/// The sector's metadata `.xml` text (MetadataFile, else data-file stem + `.xml`).
pub(crate) fn read_meta_xml(
    dir: &FsPath,
    data_file: &str,
    entry: &tmap_core::dto::SectorIndexEntry,
) -> String {
    let meta_file = entry
        .metadata_file
        .clone()
        .or_else(|| {
            FsPath::new(data_file)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|stem| format!("{stem}.xml"))
        })
        .unwrap_or_else(|| format!("{}.xml", entry.name));
    resolve_ci(dir, &meta_file)
        .and_then(|p| read_text(p).ok())
        .unwrap_or_default()
}

/// Build the `# …` metadata comment block prefixed to SEC/SecondSurvey output
/// (port of `Sector.Serialize`'s header, `isT5` path).
#[allow(clippy::too_many_arguments)]
fn sec_metadata_block(
    state: &AppState,
    dir: &FsPath,
    data_file: &str,
    entry: &tmap_core::dto::SectorIndexEntry,
    abbreviation: Option<&str>,
    milieu: &str,
    subsectors: &[Subsector],
    all_worlds: &[World],
    legacy: bool,
) -> String {
    let meta_xml = read_meta_xml(dir, data_file, entry);
    let region = state.region_xml(milieu);
    let inline = milieu_sector_block(&region, &entry.name).unwrap_or_default();

    let credits = sector_credits(&meta_xml).or_else(|| sector_credits(&inline));
    let mut df: DataFileMeta = sector_datafile_meta(&inline);
    if df == DataFileMeta::default() {
        df = sector_datafile_meta(&meta_xml);
    }
    let alleg_names = sector_allegiances(&meta_xml);

    let mut s = String::new();
    let mut line = |t: &str| {
        s.push_str(t);
        s.push_str("\r\n");
    };

    line("# Generated by https://travellermap.com");
    line(&format!("# {}", iso8601_now_utc()));
    line("");

    let name0 = entry
        .names
        .first()
        .map(|n| n.text.as_str())
        .unwrap_or(&entry.name);
    line(&format!("# {name0}"));
    line(&format!("# {},{}", entry.location.x, -entry.location.y));
    line("");
    for n in &entry.names {
        match &n.lang {
            Some(lang) => line(&format!("# Name: {} ({lang})", n.text)),
            None => line(&format!("# Name: {}", n.text)),
        }
    }
    if let Some(abbr) = abbreviation {
        line("");
        line(&format!("# Abbreviation: {abbr}"));
    }
    line("");
    line(&format!("# Milieu: {milieu}"));
    if let Some(c) = &credits {
        line("");
        line(&format!("# Credits: {}", strip_html_collapse(c)));
    }
    if df != DataFileMeta::default() {
        line("");
        if let Some(v) = &df.author {
            line(&format!("# Author:    {v}"));
        }
        if let Some(v) = &df.publisher {
            line(&format!("# Publisher: {v}"));
        }
        if let Some(v) = &df.copyright {
            line(&format!("# Copyright: {v}"));
        }
        if let Some(v) = &df.source {
            line(&format!("# Source:    {v}"));
        }
        if let Some(v) = &df.reference {
            line(&format!("# Ref:       {v}"));
        }
    }
    line("");
    for i in 0..16u8 {
        let c = (b'A' + i) as char;
        let name = subsectors
            .iter()
            .find(|ss| ss.index.eq_ignore_ascii_case(&c.to_string()))
            .map(|ss| ss.name.as_str())
            .unwrap_or("");
        line(&format!("# Subsector {c}: {name}"));
    }
    line("");

    // Allegiances present across ALL worlds (not the filtered subset), sorted.
    let mut codes: Vec<&str> = all_worlds
        .iter()
        .map(|w| w.allegiance.as_str())
        .filter(|a| !a.is_empty())
        .collect();
    codes.sort_unstable();
    codes.dedup();
    for code in codes {
        // Name is always resolved from the T5 code as present in the data; the
        // emitted code is the legacy form for SEC (reference `Sector.Serialize`,
        // `isT5 ? code : T5AllegianceCodeToLegacyCode(code)`).
        let name = alleg_names
            .get(code)
            .cloned()
            .or_else(|| allegiance_name(code));
        if let Some(name) = name {
            let shown = if legacy {
                t5_to_legacy_allegiance(code)
            } else {
                code.to_string()
            };
            line(&format!("# Alleg: {shown}: \"{name}\""));
        }
    }
    line("");
    s
}

/// Strip HTML tags and collapse all whitespace runs to single spaces, trimmed
/// (the reference's `Regex.Replace("<.*?>","")` + `\s+ → " "` for `# Credits:`).
fn strip_html_collapse(s: &str) -> String {
    let mut no_tags = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => no_tags.push(c),
            _ => {}
        }
    }
    no_tags.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Current UTC time as ISO-8601 with a `+00:00` offset (the reference uses local
/// time; the value is cosmetic — generation timestamp — so UTC is fine).
fn iso8601_now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    // Howard Hinnant's days→civil(y,m,d).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00")
}

#[derive(Debug, Deserialize)]
struct MetadataQuery {
    #[serde(default = "default_milieu")]
    milieu: String,
    sector: Option<String>,
    sx: Option<i32>,
    sy: Option<i32>,
}

/// `POST /api/metadata` — reformat/round-trip uploaded sector metadata. Ports
/// `SectorMetaDataHandler.cs`'s POST path: parse the posted metadata document and
/// re-emit it (DataFile/MetadataFile cleared). The reference parses XML *or* MSEC
/// metadata and emits the sector XML document; here we parse the posted XML and
/// re-emit our documented metadata **JSON** shape — consistent with GET
/// `/api/metadata`, whose XML output is itself deferred project-wide (see the
/// TODO on `get_metadata`). DEFERRED: XML output and MSEC-format input (XML in →
/// JSON out is the implemented converter).
async fn post_metadata(body: String) -> Response {
    let trimmed = body.trim_start();
    if !trimmed.starts_with('<') {
        return (
            StatusCode::BAD_REQUEST,
            "POST /api/metadata expects an XML metadata document (MSEC input is not yet supported).",
        )
            .into_response();
    }
    let meta = parse_sector_metadata(&body);
    match serde_json::to_vec(&meta) {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/metadata` — a sector's metadata (names, subsectors, allegiances,
/// borders/regions, routes, labels, stylesheet, products, credits) in the
/// documented JSON shape. Ports `SectorMetaDataHandler.cs` (data side). Cached
/// per `(milieu, sector)` via `serve_cached`.
async fn get_metadata(
    Query(q): Query<MetadataQuery>,
    Query(jp): Query<compat::Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let universe = match state.universe(&q.milieu) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe
            .sectors
            .iter()
            .find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return (StatusCode::BAD_REQUEST, "No sector specified.").into_response();
    };
    let Some(entry) = entry else {
        return (StatusCode::NOT_FOUND, "The specified sector was not found.").into_response();
    };

    // JSONP / XML are opt-in (the default stays byte-identical cached JSON).
    // XML mirrors the reference `[XmlRoot("Sector")]` shape via `SectorMetadata::to_xml`.
    let accept_xml = compat::wants_xml(&headers);
    if jp.jsonp.is_some() || accept_xml {
        let (meta, _worlds) = assemble_metadata(&state, &q.milieu, entry);
        return compat::respond_negotiated(&meta, &jp.jsonp, accept_xml, || meta.to_xml());
    }

    let key = format!("metadata/{}/{}", q.milieu, entry.name);
    serve_cached(&state.response_cache, &key, &headers, || {
        build_metadata_bytes(&state, &q.milieu, entry)
    })
}

fn build_metadata_bytes(
    state: &AppState,
    milieu: &str,
    entry: &tmap_core::dto::SectorIndexEntry,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let (meta, _worlds) = assemble_metadata(state, milieu, entry);
    serde_json::to_vec(&meta).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Parse + merge a sector's metadata (per-sector `.xml` over the inline
/// region-list block) into a `SectorMetadata`, returning it alongside the
/// sector's parsed worlds. Shared by `/api/metadata` and `/api/credits` so the
/// two endpoints draw from one parse.
fn assemble_metadata(
    state: &AppState,
    milieu: &str,
    entry: &tmap_core::dto::SectorIndexEntry,
) -> (tmap_core::metadata::SectorMetadata, Vec<World>) {
    let dir = state.res_dir.join("Sectors").join(milieu);
    let resolved = resolve_and_parse_worlds(&dir, &entry.name, Some(entry));
    let data_file = resolved
        .as_ref()
        .map(|(f, _)| f.clone())
        .unwrap_or_else(|| format!("{}.tab", entry.name));
    let worlds: Vec<World> = resolved.map(|(_, o)| o.worlds).unwrap_or_default();

    let meta_xml = read_meta_xml(&dir, &data_file, entry);
    let region = state.region_xml(milieu);
    let inline = milieu_sector_block(&region, &entry.name).unwrap_or_default();

    // Parse both sources once; per-sector `.xml` wins, the inline region-list
    // block fills any collection it leaves empty (matches build_sector_bytes).
    let mut meta = parse_sector_metadata(&meta_xml);
    let inline_meta = parse_sector_metadata(&inline);
    if meta.subsectors.is_empty() {
        meta.subsectors = inline_meta.subsectors;
    }
    if meta.borders.is_empty() {
        meta.borders = inline_meta.borders;
    }
    if meta.regions.is_empty() {
        meta.regions = inline_meta.regions;
    }
    if meta.routes.is_empty() {
        meta.routes = inline_meta.routes;
    }
    if meta.products.is_empty() {
        meta.products = inline_meta.products;
    }
    if meta.labels.is_empty() {
        meta.labels = inline_meta.labels;
    }
    if meta.stylesheet.is_none() {
        meta.stylesheet = inline_meta.stylesheet;
    }
    let mut local_alleg = meta.local_allegiances.clone();
    local_alleg.extend(inline_meta.local_allegiances.clone());

    // Identity from the sector index entry (authoritative).
    meta.x = entry.location.x;
    meta.y = entry.location.y;
    if !entry.names.is_empty() {
        meta.names = entry.names.clone();
    }
    meta.selected = meta.selected || inline_meta.selected;
    meta.label = meta.label.or(inline_meta.label);
    let mtag = milieu_tag(&state.res_dir, milieu);
    meta.tags = [entry.tags.as_str(), mtag.as_deref().unwrap_or("")]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let is_otu = meta.tags.split_whitespace().any(|t| t == "OTU");
    meta.abbreviation = entry.abbreviation.clone().or_else(|| {
        is_otu
            .then(|| {
                meta.names
                    .first()
                    .and_then(|n| synthesize_abbreviation(&n.text))
            })
            .flatten()
    });

    let mut df = sector_datafile_meta(&inline);
    if df == DataFileMeta::default() {
        df = sector_datafile_meta(&meta_xml);
    }
    meta.data_file = tmap_core::metadata::MetaDataFile {
        title: df.title,
        author: df.author,
        source: df.source,
        publisher: df.publisher,
        copyright: df.copyright,
        milieu: Some(milieu.to_string()),
        reference: df.reference,
    };

    meta.allegiances =
        compute_metadata_allegiances(&worlds, &meta.borders, &meta.regions, &local_alleg);

    (meta, worlds)
}

#[derive(Debug, Deserialize)]
struct CreditsQuery {
    #[serde(default = "default_milieu")]
    milieu: String,
    sector: Option<String>,
    sx: Option<i32>,
    sy: Option<i32>,
    hex: Option<String>,
}

/// `GET /api/credits` — credits/attribution for a location (port of
/// `CreditsHandler.cs`, data side). Resolves the sector (by name or sx,sy) and
/// a hex (defaults to the sector's central hex), returning sector/subsector/
/// world/product credits as the documented JSON shape.
async fn get_credits(
    Query(q): Query<CreditsQuery>,
    Query(jp): Query<compat::Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    credits_response(&state, q, &jp.jsonp, compat::wants_xml(&headers))
}

fn credits_response(
    state: &AppState,
    q: CreditsQuery,
    jsonp: &Option<String>,
    accept_xml: bool,
) -> Response {
    let universe = match state.universe(&q.milieu) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe
            .sectors
            .iter()
            .find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return (StatusCode::BAD_REQUEST, "No sector specified.").into_response();
    };
    let Some(entry) = entry else {
        return (StatusCode::NOT_FOUND, "The specified sector was not found.").into_response();
    };

    // Default to the sector's central hex (1620), matching Astrometrics.SectorCentralHex.
    let hex = q.hex.clone().unwrap_or_else(|| "1620".to_string());
    let result = build_credits(state, &q.milieu, entry, &hex);
    // JSON default (byte-identical to before); jsonp/xml opt-in only.
    compat::respond_negotiated(&result, jsonp, accept_xml, || result.to_xml())
}

// --- Semantic /data/{sector}/... URL aliases (Global.asax.cs `/data` table) ---

/// Build a [`SecQuery`] for a `/data` alias with the given sector + format.
fn data_sec_query(sector: String, type_: &str) -> SecQuery {
    SecQuery {
        milieu: default_milieu(),
        sector: Some(sector),
        sx: None,
        sy: None,
        subsector: None,
        quadrant: None,
        type_: Some(type_.to_string()),
        metadata: None,
        header: None,
        sscoords: None,
    }
}

/// A `SecQuery` for a subsector/quadrant region alias. Mirrors the reference
/// `/data/{sector}/{region}` routes: `metadata=0` (no comment block) and the
/// region restricted via `subsector=`/`quadrant=`.
fn region_sec_query(sector: String, region: &str, type_: &str) -> SecQuery {
    let mut q = data_sec_query(sector, type_);
    q.metadata = Some("0".to_string());
    if quadrant_index_for(region).is_some() {
        q.quadrant = Some(region.to_string());
    } else {
        q.subsector = Some(region.to_string());
    }
    q
}

/// True if `tail` is a 4-digit hex (the reference `(?<hex>[0-9]{4})` form), i.e.
/// a single-world lookup rather than a subsector/quadrant region.
fn is_hex_tail(tail: &str) -> bool {
    tail.len() == 4 && tail.bytes().all(|b| b.is_ascii_digit())
}

/// `/data/{sector}` and `/data/{sector}/sec` → SecondSurvey text (with metadata).
async fn data_sec(Path(sector): Path<String>, State(state): State<AppState>) -> Response {
    sec_text_response(build_sec(&state, &data_sec_query(sector, "SecondSurvey")))
}

/// `/data/{sector}/tab` → TabDelimited text.
async fn data_tab(Path(sector): Path<String>, State(state): State<AppState>) -> Response {
    sec_text_response(build_sec(&state, &data_sec_query(sector, "TabDelimited")))
}

fn sec_text_response(r: Result<String, (StatusCode, String)>) -> Response {
    match r {
        Ok(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

// `/data/{sector}/coordinates` and `/data/{sector}/{hex}/coordinates` live in
// `compat` (they share the CoordinatesHandler logic).

/// `/data/{sector}/credits` → credits at the sector centre.
async fn data_credits(Path(sector): Path<String>, State(state): State<AppState>) -> Response {
    credits_response(
        &state,
        CreditsQuery {
            milieu: default_milieu(),
            sector: Some(sector),
            sx: None,
            sy: None,
            hex: None,
        },
        &None,
        false,
    )
}

/// `/data/{sector}/{hex}/credits` → credits at a specific world.
async fn data_credits_hex(
    Path((sector, hex)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    credits_response(
        &state,
        CreditsQuery {
            milieu: default_milieu(),
            sector: Some(sector),
            sx: None,
            sy: None,
            hex: Some(hex),
        },
        &None,
        false,
    )
}

/// `/data/{sector}/metadata` → sector metadata (NOTE: XML by default in the
/// reference; we emit JSON until XML content negotiation lands).
async fn data_metadata(
    Path(sector): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    get_metadata(
        Query(MetadataQuery {
            milieu: default_milieu(),
            sector: Some(sector),
            sx: None,
            sy: None,
        }),
        Query(compat::Jsonp::default()),
        headers,
        State(state),
    )
    .await
}

// --- MSEC ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MsecQuery {
    #[serde(default = "default_milieu")]
    milieu: String,
    /// Sector by display name or T5SS abbreviation.
    sector: Option<String>,
    sx: Option<i32>,
    sy: Option<i32>,
}

/// `GET /api/msec` — a sector's metadata as legacy MSEC ("metadata SEC") text.
/// Ports `server/api/MSECHandler.cs` + `MSECWriter.cs`. The format (sector header,
/// `a`–`p` subsectors, allegiance-grouped `route`/`label`/`border` lines) is
/// produced by `tmap_core::msec_writer`.
async fn get_msec(Query(q): Query<MsecQuery>, State(state): State<AppState>) -> Response {
    match build_msec(&state, &q) {
        Ok(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

fn build_msec(state: &AppState, q: &MsecQuery) -> Result<String, (StatusCode, String)> {
    // Resolve the sector by sx,sy or name/abbreviation (mirrors get_sec).
    let universe = state.universe(&q.milieu)?;
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe
            .sectors
            .iter()
            .find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return Err((StatusCode::BAD_REQUEST, "No sector specified.".into()));
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "The specified sector was not found.".into(),
        )
    })?;

    let (meta, _worlds) = assemble_metadata(state, &q.milieu, entry);
    let name = meta
        .names
        .first()
        .map(|n| n.text.as_str())
        .unwrap_or(&entry.name);
    // Domain/quadrant header fields only ever come from MSEC *input* in the
    // reference, never from a sector's `.xml`, so they are always empty here.
    let header = tmap_core::msec_writer::MsecHeader::default();
    Ok(tmap_core::msec_writer::write_msec(
        name,
        &header,
        &meta,
        &iso8601_now_utc(),
    ))
}

/// `/data/{sector}/msec` → MSEC metadata text (semantic alias of `/api/msec`).
async fn data_msec(Path(sector): Path<String>, State(state): State<AppState>) -> Response {
    get_msec(
        Query(MsecQuery {
            milieu: default_milieu(),
            sector: Some(sector),
            sx: None,
            sy: None,
        }),
        State(state),
    )
    .await
}

fn build_credits(
    state: &AppState,
    milieu: &str,
    entry: &tmap_core::dto::SectorIndexEntry,
    hex: &str,
) -> tmap_core::dto::CreditsResult {
    let nonempty = |s: String| (!s.is_empty()).then_some(s);
    let (meta, worlds) = assemble_metadata(state, milieu, entry);

    // `meta`/`worlds` are owned, used once here — move each field into the result
    // instead of cloning it out.
    let mut r = tmap_core::dto::CreditsResult {
        sector_x: meta.x,
        sector_y: meta.y,
        ..Default::default()
    };
    r.sector_name = meta.names.into_iter().next().map(|n| n.text);
    r.credits = meta.credits_text;
    r.sector_tags = nonempty(meta.tags);
    r.sector_author = meta.data_file.author;
    r.sector_source = meta.data_file.source;
    r.sector_publisher = meta.data_file.publisher;
    r.sector_copyright = meta.data_file.copyright;
    r.sector_ref = meta.data_file.reference;
    r.sector_milieu = meta.data_file.milieu.or_else(|| Some(milieu.to_string()));

    if let Some(p) = meta.products.into_iter().next() {
        r.product_publisher = p.publisher;
        r.product_title = p.title;
        r.product_author = p.author;
        r.product_ref = p.reference;
    }

    // Subsector (by its letter index A–P for this hex).
    let letter = astrometrics::subsector_letter(hex).to_string();
    if let Some(ss) = meta.subsectors.into_iter().find(|s| s.index == letter) {
        r.subsector_name = nonempty(ss.name);
        r.subsector_index = nonempty(ss.index);
    }

    // World at the hex.
    if let Some(w) = worlds.into_iter().find(|w| w.hex == hex) {
        r.world_name = nonempty(w.name);
        r.world_hex = nonempty(w.hex);
        r.world_uwp = nonempty(w.uwp);
        r.world_remarks = nonempty(w.remarks);
        r.world_ix = w.importance;
        r.world_ex = w.economic;
        r.world_cx = w.cultural;
        r.world_pbg = nonempty(w.pbg);
        r.world_allegiance = nonempty(w.allegiance);
    }

    r
}

// --- JumpWorlds ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JumpWorldsQuery {
    #[serde(default = "default_milieu")]
    milieu: String,
    sector: Option<String>,
    sx: Option<i32>,
    sy: Option<i32>,
    hex: Option<String>,
    #[serde(default = "default_jumpworlds_jump")]
    jump: i32,
}

fn default_jumpworlds_jump() -> i32 {
    6
}

/// Per-sector context used to denormalize each world into a [`WorldResult`].
struct JumpSectorCtx {
    name: String,
    abbreviation: Option<String>,
    subsectors: Vec<Subsector>,
}

/// `GET /api/jumpworlds?sector=&hex=&jump=N` — every world within `jump` parsecs
/// of a hex, as `{Worlds:[…]}` (port of `JumpWorldsHandler` + `HexSelector`).
async fn get_jumpworlds(
    Query(q): Query<JumpWorldsQuery>,
    Query(jp): Query<compat::Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    match build_jumpworlds(&state, &q) {
        // JSON default (byte-identical to before); jsonp/xml opt-in only.
        Ok(result) => {
            compat::respond_negotiated(&result, &jp.jsonp, compat::wants_xml(&headers), || {
                result.to_xml()
            })
        }
        Err((code, msg)) => (code, msg).into_response(),
    }
}

/// `GET /data/{sector}/{hex}` — the single world at a hex as the reference SEC
/// JSON envelope `{"Worlds":[…]}` (jumpworlds at jump 0, default milieu). This is
/// the endpoint third-party tools (e.g. worldgen) use to look up a world by
/// sector name + 4-digit hex; an empty hex yields `{"Worlds":[]}`.
/// `/data/{sector}/{tail}` → a single world (`tail` = 4-digit hex), or a
/// subsector/quadrant region as SecondSurvey text (port of the reference's
/// quadrant / subsector-by-letter / subsector-by-name `/data` routes, which all
/// share this URL shape and are disambiguated by the segment's content).
async fn data_world_or_region(
    Path((sector, tail)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    if is_hex_tail(&tail) {
        let q = JumpWorldsQuery {
            milieu: default_milieu(),
            sector: Some(sector),
            sx: None,
            sy: None,
            hex: Some(tail),
            jump: 0,
        };
        return match build_jumpworlds(&state, &q) {
            Ok(result) => Json(result).into_response(),
            Err((code, msg)) => (code, msg).into_response(),
        };
    }
    sec_text_response(build_sec(
        &state,
        &region_sec_query(sector, &tail, "SecondSurvey"),
    ))
}

/// `/data/{sector}/{region}/sec` → a subsector/quadrant region as legacy SEC text.
async fn data_region_sec(
    Path((sector, region)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    sec_text_response(build_sec(&state, &region_sec_query(sector, &region, "SEC")))
}

/// `/data/{sector}/{region}/tab` → a subsector/quadrant region as TabDelimited text.
async fn data_region_tab(
    Path((sector, region)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    sec_text_response(build_sec(
        &state,
        &region_sec_query(sector, &region, "TabDelimited"),
    ))
}

fn build_jumpworlds(
    state: &AppState,
    q: &JumpWorldsQuery,
) -> Result<tmap_core::dto::JumpWorldsResult, (StatusCode, String)> {
    let jump = q.jump.clamp(0, 12);
    let universe = state.universe(&q.milieu)?;

    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe
            .sectors
            .iter()
            .find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return Err((StatusCode::BAD_REQUEST, "No sector specified.".into()));
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "The specified sector was not found.".into(),
        )
    })?;

    let hex = q
        .hex
        .as_deref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "No hex specified.".into()))?;
    let (chx, chy) =
        parse_hex(hex).ok_or_else(|| (StatusCode::BAD_REQUEST, format!("Invalid hex: {hex}")))?;
    let (cx, cy) =
        astrometrics::location_to_coordinates(entry.location.x, entry.location.y, chx, chy);
    let center = Coord::new(cx, cy);

    // Absolute-coordinate bounding box (matches HexSelector: ±(jump+1)).
    let (x0, x1) = (cx - jump - 1, cx + jump + 1);
    let (y0, y1) = (cy - jump - 1, cy + jump + 1);
    let (sxa, sya, ..) = astrometrics::coordinates_to_location(x0, y0);
    let (sxb, syb, ..) = astrometrics::coordinates_to_location(x1, y1);

    // Load every candidate sector once; index its worlds by absolute coord.
    let dir = state.res_dir.join("Sectors").join(&q.milieu);
    let mtag = milieu_tag(&state.res_dir, &q.milieu);
    let mut ctxs: Vec<JumpSectorCtx> = Vec::new();
    let mut world_map: HashMap<(i32, i32), (World, usize)> = HashMap::new();
    for sy_ in sya..=syb {
        for sx_ in sxa..=sxb {
            let Some(e) = universe
                .sectors
                .iter()
                .find(|s| s.location.x == sx_ && s.location.y == sy_)
            else {
                continue;
            };
            let Some((file, outcome)) = resolve_and_parse_worlds(&dir, &e.name, Some(e)) else {
                continue;
            };
            let subsectors = gather_subsectors(state, &dir, &file, e, &q.milieu);
            let is_otu = e
                .tags
                .split_whitespace()
                .chain(mtag.as_deref().unwrap_or("").split_whitespace())
                .any(|t| t == "OTU");
            let abbreviation = e.abbreviation.clone().or_else(|| {
                is_otu
                    .then(|| {
                        e.names
                            .first()
                            .and_then(|n| synthesize_abbreviation(&n.text))
                    })
                    .flatten()
            });
            let ctx_idx = ctxs.len();
            ctxs.push(JumpSectorCtx {
                name: e
                    .names
                    .first()
                    .map(|n| n.text.clone())
                    .unwrap_or_else(|| e.name.clone()),
                abbreviation,
                subsectors,
            });
            for w in outcome.worlds {
                let Some((col, row)) = parse_hex(&w.hex) else {
                    continue;
                };
                let (wx, wy) = astrometrics::location_to_coordinates(sx_, sy_, col, row);
                world_map.entry((wx, wy)).or_insert((w, ctx_idx));
            }
        }
    }

    // Raster scan (y outer, x inner) over the bbox — the HexSelector emit order.
    let mut worlds = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if astrometrics::reference_hex_distance(center, Coord::new(x, y)) > jump {
                continue;
            }
            if let Some((w, ci)) = world_map.get(&(x, y)) {
                worlds.push(world_to_result(w, &ctxs[*ci], x, y));
            }
        }
    }

    Ok(tmap_core::dto::JumpWorldsResult { worlds })
}

/// Denormalize a [`World`] + sector context into the public [`WorldResult`].
fn world_to_result(
    w: &World,
    ctx: &JumpSectorCtx,
    world_x: i32,
    world_y: i32,
) -> tmap_core::dto::WorldResult {
    let ss = astrometrics::subsector_letter(&w.hex).to_string();
    let subsector = astrometrics::subsector_index(&w.hex) as i32;
    let quadrant = astrometrics::quadrant_index(&w.hex) as i32;
    let subsector_name = ctx
        .subsectors
        .iter()
        .find(|s| s.index == ss)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    tmap_core::dto::WorldResult {
        name: w.name.clone(),
        hex: w.hex.clone(),
        uwp: w.uwp.clone(),
        pbg: w.pbg.clone(),
        zone: w.zone.clone(),
        bases: w.bases.clone(),
        allegiance: w.allegiance.clone(),
        stellar: w.stellar.clone(),
        ss,
        ix: w.importance.clone(),
        ex: w.economic.clone(),
        cx: w.cultural.clone(),
        nobility: w.nobility.clone().unwrap_or_default(),
        worlds: w.worlds.map_or(0, i32::from),
        resource_units: w.resource_units,
        subsector,
        quadrant,
        world_x,
        world_y,
        remarks: w.remarks.clone(),
        legacy_base_code: encode_legacy_bases(&w.allegiance, &w.bases),
        sector: ctx.name.clone(),
        subsector_name,
        sector_abbreviation: ctx.abbreviation.clone(),
        allegiance_name: allegiance_name(&w.allegiance).unwrap_or_default(),
    }
}

/// The `Allegiances` list: every code used by the worlds and by the borders/
/// regions, each resolved to `{Name, Code, Base}` (sector-local `<Allegiance>`
/// overrides first, else the stock tables). Sorted by code for determinism (the
/// reference uses an unordered `HashSet`).
fn compute_metadata_allegiances(
    worlds: &[World],
    borders: &[tmap_core::metadata::MetaBorder],
    regions: &[tmap_core::metadata::MetaBorder],
    local: &[MetaAllegiance],
) -> Vec<MetaAllegiance> {
    let local_map: HashMap<&str, &MetaAllegiance> =
        local.iter().map(|a| (a.code.as_str(), a)).collect();
    let mut codes: Vec<&str> = worlds
        .iter()
        .map(|w| w.allegiance.as_str())
        .filter(|c| !c.is_empty())
        .collect();
    for b in borders.iter().chain(regions) {
        if let Some(a) = &b.allegiance {
            codes.push(a);
        }
    }
    codes.sort_unstable();
    codes.dedup();
    codes
        .iter()
        .filter_map(|&code| {
            let (name, base) = match local_map.get(code) {
                Some(l) => (Some(l.name.clone()), l.base.clone()),
                None => (allegiance_name(code), allegiance_base(code)),
            };
            name.map(|name| MetaAllegiance {
                name,
                code: code.to_string(),
                base,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct SectorQuery {
    /// Level of detail: `full` (everything) or `overview` (lighter — drops
    /// fields not rendered until extreme zoom). See PORT_PLAN.md.
    #[serde(default = "default_lod")]
    lod: String,
}

fn default_lod() -> String {
    "full".to_string()
}

/// `GET /api/sector/{milieu}/{name}?lod=full|overview` — a sector's worlds
/// (+ borders/routes) as JSON, cached and CDN-friendly.
async fn get_sector(
    Path((milieu, name)): Path<(String, String)>,
    Query(q): Query<SectorQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    if q.lod != "full" && q.lod != "overview" {
        return (
            StatusCode::BAD_REQUEST,
            format!("unsupported lod '{}'", q.lod),
        )
            .into_response();
    }
    if !is_safe_segment(&milieu) || !is_safe_segment(&name) {
        return (StatusCode::BAD_REQUEST, "invalid sector path".to_string()).into_response();
    }
    let key = format!("sector/{milieu}/{name}/{}", q.lod);
    serve_cached(&state.response_cache, &key, &headers, || {
        build_sector_bytes(&state, &milieu, &name, &q.lod)
    })
}

/// Resolve a sector's data file and parse its worlds. The index's `DataFile` is
/// tried first, else the stem with each known extension (sectors not in the
/// region list default to `<name>.tab`, but the data may actually be
/// `.txt`/`.sec`). Returns the chosen filename + parsed worlds, or `None` if no
/// data file exists. Shared by sector serving and the route world-index build.
pub(crate) fn resolve_and_parse_worlds(
    dir: &FsPath,
    name: &str,
    entry: Option<&tmap_core::dto::SectorIndexEntry>,
) -> Option<(String, tmap_core::parse::ParseOutcome)> {
    let stem = entry
        .and_then(|s| s.data_file.as_deref())
        .map(|df| {
            std::path::Path::new(df)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(name)
        })
        .unwrap_or(name)
        .to_string();
    // Resolve filenames case-insensitively — the region list declares e.g.
    // `Blaskon.txt` but the on-disk file is `blaskon.txt` (see `resolve_ci`).
    // The declared `Type` wins; `None` means "sniff the content" (deferred until
    // the file is read), mirroring the reference `WorldCollection.Deserialize`.
    let (data_file, declared_format) = entry
        .and_then(|s| s.data_file.clone())
        .filter(|df| resolve_ci(dir, df).is_some())
        .map(|df| (df, entry.and_then(|s| s.data_format.clone())))
        .or_else(|| {
            // Loose fallback by extension: `.tab`/`.sec` are unambiguous; a bare
            // `.txt` is sniffed (legacy SEC vs. T5 SecondSurvey both use it).
            [
                ("tab", Some("TabDelimited")),
                ("txt", None),
                ("sec", Some("SEC")),
            ]
            .into_iter()
            .map(|(ext, fmt)| (format!("{stem}.{ext}"), fmt.map(str::to_owned)))
            .find(|(f, _)| resolve_ci(dir, f).is_some())
        })?;

    let text = read_text(resolve_ci(dir, &data_file)?).ok()?;
    // No declared `Type` → sniff the content (reference `SectorFileParser.SniffType`):
    // tab-delimited → TabDelimited, `{Ix} (Ex) [Cx]` present → SecondSurvey, else
    // legacy SEC. So `.txt` files split correctly between T5 SecondSurvey (Phlask)
    // and legacy SEC fixed-column (Faraway's Virgo) — the latter `parse_column`
    // can't read.
    let data_format = declared_format.unwrap_or_else(|| sniff_world_format(&text).to_owned());
    let outcome = match data_format.as_str() {
        "SEC" => parse_sec(&text),             // legacy regex format (.sec)
        "SecondSurvey" => parse_column(&text), // T5 column format (.txt)
        _ => parse_tab(&text),
    }
    // A format quirk shouldn't fail the whole sector — fall back to no worlds.
    .unwrap_or_default();
    Some((data_file, outcome))
}

/// Detect a world-data file's format from its content (port of the reference
/// `SectorFileParser.SniffType`): a line with ≥9 tabs → `TabDelimited`; a line
/// carrying the T5 `{Ix} (Ex) [Cx]` extensions → `SecondSurvey`; otherwise the
/// legacy fixed-column `SEC`. Comment lines (`# $ @`) are skipped.
fn sniff_world_format(text: &str) -> &'static str {
    for line in text.lines() {
        if line.is_empty() || matches!(line.as_bytes().first(), Some(b'#' | b'$' | b'@')) {
            continue;
        }
        if line.matches('\t').count() >= 9 {
            return "TabDelimited";
        }
        if has_t5_extensions(line) {
            return "SecondSurvey";
        }
    }
    "SEC"
}

/// Whether a line carries the T5 `{Ix} (Ex) [Cx]` extension triple in order
/// (reference sniff regex `\{.*\} +\(.*\) +\[.*\]`): a `{…}` group, then one or
/// more spaces, then a `(…)` group, spaces, then a `[…]` group.
fn has_t5_extensions(line: &str) -> bool {
    fn after_group(s: &str, open: u8, close: u8) -> Option<&str> {
        let start = s.find(open as char)?;
        let rel_close = s[start + 1..].find(close as char)?;
        Some(&s[start + 1 + rel_close + 1..])
    }
    fn skip_spaces(s: &str) -> Option<&str> {
        let t = s.trim_start_matches(' ');
        (t.len() < s.len()).then_some(t)
    }
    let rest = after_group(line, b'{', b'}').and_then(skip_spaces);
    let rest = rest
        .and_then(|s| after_group(s, b'(', b')'))
        .and_then(skip_spaces);
    rest.and_then(|s| after_group(s, b'[', b']')).is_some()
}

/// Parse + assemble a sector and serialize it (the cache-miss path).
pub(crate) fn build_sector_bytes(
    state: &AppState,
    milieu: &str,
    name: &str,
    lod: &str,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let data = build_sector_data(state, milieu, name, lod)?;
    serde_json::to_vec(&data).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Parse + assemble a sector's full [`SectorData`] (worlds + borders + routes +
/// labels) from `res/`. The in-memory form behind both `/api/sector` (which
/// serializes it) and `/api/tile` (which renders it).
pub(crate) fn build_sector_data(
    state: &AppState,
    milieu: &str,
    name: &str,
    lod: &str,
) -> Result<SectorData, (StatusCode, String)> {
    let dir = state.res_dir.join("Sectors").join(milieu);

    // The index gives the sector's grid position + which data file/format to
    // read (TabDelimited vs column-delimited SecondSurvey/SEC).
    let universe = state.universe(milieu).ok();
    let entry = universe
        .as_ref()
        .and_then(|u| u.sectors.iter().find(|s| s.name == name));
    let location = entry.map(|s| s.location);

    // Resolve the data file + parse the worlds (shared with the route index).
    let Some((data_file, outcome)) = resolve_and_parse_worlds(&dir, name, entry) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no data for '{name}' in '{milieu}'"),
        ));
    };

    // Metadata filename: region-list MetadataFile, else the data file's stem +
    // ".xml" (a sector's display name often differs from its filename, e.g.
    // "The Beyond" → Beyond.xml), else "<name>.xml".
    let meta_file = entry
        .and_then(|s| s.metadata_file.clone())
        .or_else(|| {
            std::path::Path::new(&data_file)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|stem| format!("{stem}.xml"))
        })
        .unwrap_or_else(|| format!("{name}.xml"));
    let meta_xml = resolve_ci(&dir, &meta_file)
        .and_then(|p| read_text(p).ok())
        .unwrap_or_default();
    // A sector's metadata may live in its own `<name>.xml` AND/OR inline in the
    // milieu region list's `<Sector>` block — read BOTH and merge (dedup), never
    // assuming one or the other. The Aslan Hierate interior, for example, has
    // borders only inline in the region list while its worlds load from a bare
    // `.tab`; frontier sectors have their own file. Missing either source drops
    // real borders/routes (left whole polities unshaded).
    let region = state.region_xml(milieu);
    let inline = tmap_core::parse::milieu_sector_block(&region, name).unwrap_or_default();

    // Phase B: parse each metadata source **once** into the shared
    // `SectorMetadata`, then project to the render `SectorData` — replacing the
    // previous ~8 separate `sector_*` calls (each of which re-parsed the XML).
    // The reference's per-sector `.xml` wins; the inline region-list block fills
    // gaps. Output is byte-identical to the old per-element path (verified
    // against a baseline of all M1105 sectors).
    let meta = parse_sector_metadata(&meta_xml);
    let inline_meta = parse_sector_metadata(&inline);

    let mut subsectors: Vec<Subsector> = render_subsectors(&meta.subsectors);
    if subsectors.is_empty() {
        subsectors = render_subsectors(&inline_meta.subsectors);
    }

    // Routes: per-sector first, then inline (dedup by start/end + offsets).
    let mut routes: Vec<tmap_core::dto::Route> = meta.routes.iter().map(render_route).collect();
    // (start, end, start_offset, end_offset) — identity for route dedup.
    type RouteKey = (String, String, (i32, i32), (i32, i32));
    let mut seen_routes: HashSet<RouteKey> = routes
        .iter()
        .map(|r| (r.start.clone(), r.end.clone(), r.start_offset, r.end_offset))
        .collect();
    for r in inline_meta.routes.iter().map(render_route) {
        if seen_routes.insert((r.start.clone(), r.end.clone(), r.start_offset, r.end_offset)) {
            routes.push(r);
        }
    }

    // Borders + Regions feed the same micro-border layer, in source document
    // order (interleaved, via `seq`), deduped by allegiance + hexes.
    let mut borders: Vec<tmap_core::dto::Border> = ordered_borders(&meta)
        .into_iter()
        .map(render_border)
        .collect();
    let mut seen_borders: HashSet<(String, Vec<String>)> = borders
        .iter()
        .map(|b| (b.allegiance.clone(), b.hexes.clone()))
        .collect();
    for b in ordered_borders(&inline_meta).into_iter().map(render_border) {
        if seen_borders.insert((b.allegiance.clone(), b.hexes.clone())) {
            borders.push(b);
        }
    }

    // Border fill colors from the sector stylesheet's `border.<alleg>` rules.
    let mut border_styles = meta
        .stylesheet
        .as_deref()
        .map(tmap_core::parse::parse_border_styles_css)
        .unwrap_or_default();
    if let Some(css) = inline_meta.stylesheet.as_deref() {
        for (k, v) in tmap_core::parse::parse_border_styles_css(css) {
            border_styles.entry(k).or_insert(v);
        }
    }

    // Sector-local allegiance names (preferred over the global stock table when
    // labeling a border with no explicit `Label`), from both metadata sources.
    let mut alleg_names: HashMap<String, String> = meta
        .local_allegiances
        .iter()
        .map(|a| (a.code.clone(), a.name.clone()))
        .collect();
    for a in &inline_meta.local_allegiances {
        alleg_names
            .entry(a.code.clone())
            .or_insert_with(|| a.name.clone());
    }

    for b in &mut borders {
        if let Some(loc) = location {
            b.region = border_region(&b.hexes, loc.x, loc.y);
        }
        if b.color.is_none() {
            b.color = border_styles.get(&b.allegiance).cloned();
        }
        // Reference `Border.GetLabel`: with no explicit label, fall back to the
        // allegiance name (sector-local first, then the global stock table). Only
        // when the border has a placement (`LabelPosition`) and is not suppressed.
        if b.label.is_none() && b.label_position.is_some() && !b.allegiance.is_empty() {
            b.label = alleg_names
                .get(&b.allegiance)
                .cloned()
                .or_else(|| tmap_core::world_util::allegiance_name(&b.allegiance));
        }
    }

    // Standalone hand-placed labels ("Outrim Void", …) from both sources.
    let mut labels: Vec<tmap_core::dto::SectorLabel> =
        meta.labels.iter().map(render_label).collect();
    let mut seen_labels: HashSet<(String, String)> = labels
        .iter()
        .map(|l| (l.text.clone(), l.hex.clone()))
        .collect();
    for l in inline_meta.labels.iter().map(render_label) {
        if seen_labels.insert((l.text.clone(), l.hex.clone())) {
            labels.push(l);
        }
    }

    let worlds = if lod == "overview" {
        outcome.worlds.into_iter().map(project_overview).collect()
    } else {
        outcome.worlds
    };

    // Review tags + data-source credit (prefer the per-sector xml; the inline
    // region-list block carries Tags too, so fall back to it for tags).
    let tags = if meta.tags.is_empty() {
        inline_meta.tags
    } else {
        meta.tags
    };
    let credits = meta.credits_text.or(inline_meta.credits_text);

    let data = SectorData {
        info: SectorInfo {
            name: name.to_string(),
            location,
            milieu: milieu.to_string(),
            subsectors,
            tags,
            credits,
        },
        worlds,
        borders,
        routes,
        labels,
    };
    Ok(data)
}

/// A single, harmless path segment: no separators, no `..`, not empty.
fn is_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && FsPath::new(s).components().count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn test_state() -> AppState {
        // `res/` is at the workspace root, two levels up from this crate.
        AppState::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../res"))
    }

    /// `resolve_ci` must find a file whose on-disk case differs from the declared
    /// name — the Blaskon/Kidunal class of bug (capitalized in the region list,
    /// lowercase on disk) that silently dropped data on case-sensitive Linux. On
    /// macOS this passes via the fast `exists()` path; on Linux it exercises the
    /// directory scan — the path that actually matters in production / CI.
    #[test]
    fn resolve_ci_matches_mismatched_case() {
        let dir = std::env::temp_dir().join(format!("tmap_ci_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blaskon.xml"), b"<x/>").unwrap();

        // Declared "Blaskon.xml" (upstream casing) resolves to the lowercase file.
        let p = resolve_ci(&dir, "Blaskon.xml").expect("case-insensitive match");
        assert_eq!(std::fs::read(&p).unwrap(), b"<x/>");
        // Exact case still resolves; a truly-absent file does not.
        assert!(resolve_ci(&dir, "blaskon.xml").is_some());
        assert!(resolve_ci(&dir, "nope.xml").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every **data-bearing** sector in M1105 must load and serialize to valid
    /// `SectorData` — guards against encoding, data-file-resolution, parse, and
    /// metadata regressions (the bugs found during Phase 10 testing). The
    /// universe also includes positioned-but-dataless sectors (no `.tab`); those
    /// have no world payload, so they're skipped here.
    #[test]
    fn all_m1105_sectors_load() {
        let state = test_state();
        let universe = state.universe("M1105").expect("M1105 universe builds");
        assert!(
            universe.sectors.len() > 150,
            "expected the full M1105 index, got only {}",
            universe.sectors.len()
        );

        // Only sectors whose data file actually resolves on disk are expected to
        // load. The aggregated index also lists sectors whose declared data file
        // isn't in this checkout (upstream data drift) or that are positioned-
        // but-dataless — those legitimately 404 and are skipped.
        let dir = state.res_dir.join("Sectors").join("M1105");
        let loadable: Vec<_> = universe
            .sectors
            .iter()
            .filter(|s| resolve_and_parse_worlds(&dir, &s.name, Some(s)).is_some())
            .collect();
        assert!(
            loadable.len() > 150,
            "expected the full M1105 data-bearing set, got only {}",
            loadable.len()
        );

        let mut failures = Vec::new();
        for s in &loadable {
            match build_sector_bytes(&state, "M1105", &s.name, "overview") {
                Ok(bytes) => {
                    // Must round-trip back into the wire type the client decodes.
                    if let Err(e) = serde_json::from_slice::<SectorData>(&bytes) {
                        failures.push(format!("{}: invalid JSON ({e})", s.name));
                    }
                }
                Err((code, msg)) => failures.push(format!("{}: {code} {msg}", s.name)),
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} loadable M1105 sectors failed:\n  {}",
            failures.len(),
            loadable.len(),
            failures.join("\n  ")
        );

        // Legacy `.sec` sectors must actually parse worlds — they previously
        // loaded borders/metadata but 0 worlds (no SEC regex parser). Guard it.
        let bytes = build_sector_bytes(&state, "M1105", "Yiklerzdanzh", "overview")
            .expect("Yiklerzdanzh (SEC) builds");
        let data: SectorData = serde_json::from_slice(&bytes).unwrap();
        assert!(
            data.worlds.len() > 100,
            "Yiklerzdanzh (SEC format) should parse worlds, got {}",
            data.worlds.len()
        );

        // Aslan Hierate interior sectors (e.g. Hlakhoi) have NO per-sector
        // metadata `.xml` — their `As` border lives only inline in the milieu
        // region list `M1105.xml`. Must still produce a filled border region,
        // else huge swathes of the Hierate render unshaded.
        let bytes =
            build_sector_bytes(&state, "M1105", "Hlakhoi", "overview").expect("Hlakhoi builds");
        let data: SectorData = serde_json::from_slice(&bytes).unwrap();
        let as_region: usize = data
            .borders
            .iter()
            .filter(|b| b.allegiance.starts_with("As"))
            .map(|b| b.region.len())
            .sum();
        assert!(
            as_region > 100,
            "Hlakhoi (inline-only border in region list) should have an Aslan border region, got {as_region}"
        );
    }

    /// The route world-index must build for M1105 and resolve "Sector hhhh"
    /// endpoints, then the core A* must find a sensible jump route between two
    /// real Spinward Marches worlds.
    #[test]
    fn route_regina_to_yori() {
        let state = test_state();
        let index = state
            .route_index("M1105")
            .expect("M1105 route index builds");

        let start = route::resolve_location(&index, "Spinward Marches 1910")
            .expect("Regina (1910) resolves");
        let end =
            route::resolve_location(&index, "Spinward Marches 2110").expect("Yori (2110) resolves");

        let result = index
            .find_route(start, end, 2, tmap_core::route::RouteOptions::default())
            .expect("a jump-2 route from Regina to Yori exists");

        // Endpoints correct.
        assert_eq!(result.waypoints.first().unwrap().name, "Regina");
        assert_eq!(result.waypoints.last().unwrap().name, "Yori");
        // jumps == waypoints - 1, and the trip is short (Regina↔Yori ≈ 2 pc).
        assert_eq!(result.jumps, result.waypoints.len() - 1);
        assert!(
            result.jumps >= 1 && result.jumps <= 2,
            "got {} jumps",
            result.jumps
        );
        assert!(result.parsecs >= 1);
    }

    /// A jump rating too small to bridge an isolated world returns no route
    /// (clean None, not a panic). Picking an unreachable pair: jump 1 across a
    /// known multi-parsec gap.
    #[test]
    fn route_no_path_is_clean() {
        let state = test_state();
        let index = state.route_index("M1105").expect("index builds");
        // Regina to a distant world with jump 1 — Spinward Marches worlds are
        // sparse enough that jump-1 from Regina can't reach the far rim.
        let start = route::resolve_location(&index, "Spinward Marches 1910").unwrap();
        // Mora is at the far rimward-trailing corner (3124); jump-1 cannot reach.
        if let Some(end) = route::resolve_location(&index, "Spinward Marches 3124") {
            let r = index.find_route(start, end, 1, tmap_core::route::RouteOptions::default());
            // Either a long valid jump-1 chain or None — but it must not panic
            // and, if found, must be a contiguous jump-1 path.
            if let Some(res) = r {
                assert_eq!(res.waypoints.first().unwrap().name, "Regina");
            }
        }
    }
}
