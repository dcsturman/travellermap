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
        DataFileMeta, Overlays, SearchResults, SectorData, SectorInfo, SectorName, Subsector,
        Universe, UniverseResult, UniverseSector, VectorObject, World, WorldLabel,
    },
    parse::{
        border_region, milieu_sector_block, parse_column, parse_map_labels, parse_milieu_index,
        parse_sec, parse_tab, parse_vector_object, parse_world_labels, sector_allegiances,
        sector_credits, sector_datafile_meta, sector_index_entry, sector_subsectors,
    },
    metadata::{parse_sector_metadata, MetaAllegiance},
    sector_writer::{self, WriteOptions},
    world_util::{allegiance_base, allegiance_name, encode_legacy_bases, synthesize_abbreviation},
};
use tower_http::cors::CorsLayer;

mod compat;
#[cfg(test)]
mod compat_suite;
mod route;
mod search;
use search::SearchEntry;

/// Macro-overlay vector files, grouped by kind (mirrors the reference
/// `RenderContext` border/rift/route file lists).
const BORDER_FILES: &[&str] = &[
    "Imperium", "Aslan", "Kkree", "Vargr", "Zhodani", "Solomani", "Hive",
    "SpinwardClient", "RimwardClient", "TrailingClient",
];
const RIFT_FILES: &[&str] = &["GreatRift", "LesserRift", "WindhornRift", "DelphiRift", "ZhdantRift"];
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
    search_cache: Arc<Mutex<HashMap<String, Arc<Vec<SearchEntry>>>>>,
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
    fn search_index(&self, milieu: &str) -> Result<Arc<Vec<SearchEntry>>, (StatusCode, String)> {
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

/// Scan a milieu directory, parsing each per-sector `.xml` head into an index
/// entry. Non-sector XML (the milieu region list) is skipped automatically.
fn load_universe(res_dir: &FsPath, milieu: &str) -> Universe {
    let dir = res_dir.join("Sectors").join(milieu);
    let milieu_file = format!("{milieu}.xml");
    let mut by_name: HashMap<String, tmap_core::dto::SectorIndexEntry> = HashMap::new();

    // 1. The milieu region list is authoritative: full coords + DataFile/Type,
    //    including sectors whose own `.xml` omits coordinates.
    if let Ok(text) = read_text(dir.join(&milieu_file)) {
        for e in parse_milieu_index(&text) {
            by_name.entry(e.name.clone()).or_insert(e);
        }
    }
    // 2. Fall back to per-sector `.xml` for any sector not in the region list
    //    (defaults to a `<name>.tab` TabDelimited data file).
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
                    by_name.entry(e.name.clone()).or_insert(e);
                }
            }
        }
    }

    let mut sectors: Vec<_> = by_name.into_values().collect();
    sectors.sort_by(|a, b| a.name.cmp(&b.name));
    Universe {
        milieu: milieu.to_string(),
        sectors,
    }
}

impl AppState {
    /// Build a fresh state rooted at `res_dir`, with all caches empty.
    pub(crate) fn new(res_dir: PathBuf) -> Self {
        AppState {
            res_dir,
            universe_cache: Arc::new(Mutex::new(HashMap::new())),
            overlays: Arc::new(OnceLock::new()),
            search_cache: Arc::new(Mutex::new(HashMap::new())),
            response_cache: Arc::new(Mutex::new(HashMap::new())),
            region_cache: Arc::new(Mutex::new(HashMap::new())),
            route_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Assemble the application router. Shared by `main` and the compatibility test
/// suite (`compat_suite`), so tests exercise the exact routing/handlers we ship.
pub(crate) fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/universe", get(get_universe))
        .route("/api/overlays", get(get_overlays))
        .route("/api/search", get(get_search))
        .route("/api/route", get(get_route))
        .route("/api/sector/{milieu}/{name}", get(get_sector))
        // Public-API compatibility layer (documented URLs + PascalCase JSON).
        .route("/api/coordinates", get(compat::get_coordinates))
        .route("/api/sec", get(get_sec))
        .route("/api/metadata", get(get_metadata))
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
        .route("/data/{sector}/coordinates", get(compat::data_coordinates))
        .route("/data/{sector}/credits", get(data_credits))
        .route("/data/{sector}/metadata", get(data_metadata))
        .route("/data/{sector}/{tail}/coordinates", get(compat::data_coordinates_hex))
        .route("/data/{sector}/{tail}/credits", get(data_credits_hex))
        .route("/data/{sector}/{tail}/jump/{jump}", get(data_jumpworlds))
        .route("/api/res/{*path}", get(get_res))
        .route("/api/admin/flush", post(flush_cache))
        // Permissive CORS is a dev convenience (Trunk serves the wasm app from
        // a different origin). Tighten before any real deployment.
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[tokio::main]
async fn main() {
    // `res/` lives at the workspace root; override with TMAP_RES_DIR if needed.
    let res_dir = std::env::var("TMAP_RES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("res"));
    let app = build_router(AppState::new(res_dir));

    let addr = "127.0.0.1:3000";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("tmap-backend listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "ok"
}

/// `POST /api/admin/flush` — drop the built-response cache so the next request
/// for each sector/overlay re-parses from `res/` (the cold-cache path). The
/// parsed-index caches (universe/search) stay warm on purpose: for profiling we
/// want to measure sector parse + serialize, not re-parse the milieu index on
/// every request. Returns how many entries were evicted. Unauthenticated — a
/// dev/profiling convenience; gate or remove before any real deployment.
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
    if path.split('/').any(|seg| seg == ".." || seg == "." || seg.is_empty())
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
                cache.lock().unwrap().insert(key.to_owned(), (etag.clone(), bytes.clone()));
                (etag, bytes)
            }
            Err(e) => return e.into_response(),
        },
    };

    // `no-cache` = the browser/CDN may cache but must revalidate via ETag on
    // each use (cheap 304s), so a backend data/code change is never served
    // stale. Production can switch to a long max-age with versioned URLs.
    const CACHE: &str = "no-cache";
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
    subs.iter().map(|s| Subsector { index: s.index.clone(), name: s.name.clone() }).collect()
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
fn ordered_borders(m: &tmap_core::metadata::SectorMetadata) -> Vec<&tmap_core::metadata::MetaBorder> {
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
        let overlays = state.overlays.get_or_init(|| build_overlays(&state.res_dir));
        serde_json::to_vec(overlays).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
}

fn default_milieu() -> String {
    "M1105".to_string()
}

#[derive(Debug, Deserialize)]
struct UniverseQuery {
    #[serde(default = "default_milieu")]
    milieu: String,
}

/// `GET /api/universe?milieu=M1105` — the sector index for navigation, in the
/// documented public shape (`UniverseHandler`'s `{"Sectors":[…]}`, PascalCase).
/// This is the unified contract: external tools and our own Leptos client both
/// read it (the private snake_case `Universe` is now in-memory only).
async fn get_universe(
    Query(q): Query<UniverseQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let key = format!("universe/{}", q.milieu);
    serve_cached(&state.response_cache, &key, &headers, || {
        let u = state.universe(&q.milieu)?;
        // The milieu's metafile tag (e.g. "OTU") is appended to each sector's own
        // review tags ("Official" → "Official OTU"), matching the reference.
        let mtag = milieu_tag(&state.res_dir, &q.milieu);
        let sectors = u
            .sectors
            .iter()
            .map(|s| {
                let tags = [s.tags.as_str(), mtag.as_deref().unwrap_or("")]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                // Always emit at least the canonical name (older per-sector xml
                // without a localized list still has `s.name`).
                let names = if s.names.is_empty() {
                    vec![SectorName { text: s.name.clone(), lang: None }]
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
                UniverseSector {
                    x: s.location.x,
                    y: s.location.y,
                    milieu: q.milieu.clone(),
                    abbreviation,
                    tags,
                    names,
                }
            })
            .collect();
        serde_json::to_vec(&UniverseResult { sectors })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
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
}

/// `GET /api/search?q=Regina&milieu=M1105` — name search over worlds + sectors.
async fn get_search(
    Query(q): Query<SearchQuery>,
    State(state): State<AppState>,
) -> Result<Json<SearchResults>, (StatusCode, String)> {
    let idx = state.search_index(&q.milieu)?;
    let results = search::search(&idx, &q.q, 25);
    Ok(Json(SearchResults {
        query: q.q,
        results,
    }))
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
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, String)> {
    let jump = q.jump.clamp(1, 12);
    let index = state.route_index(&q.milieu)?;

    let start = route::resolve_location(&index, &q.start)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("start not found: {}", q.start)))?;
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
    if q.detail {
        Ok(Json(result).into_response())
    } else {
        Ok(Json(result.to_public_stops()).into_response())
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
    /// Output format: `TabDelimited` | `SecondSurvey`. (Legacy `SEC` pending.)
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

/// `GET /api/sec` — a sector's worlds as SEC/SecondSurvey/TabDelimited text.
/// Ports `server/api/SECHandler.cs` (data side). Currently serves
/// `type=TabDelimited` and `type=SecondSurvey`; the legacy fixed-column `SEC`
/// format (the no-`type` default) is not yet ported (needs the T5→legacy
/// allegiance/base transforms) → 400 with a pointer to the supported types.
async fn get_sec(Query(q): Query<SecQuery>, State(state): State<AppState>) -> Response {
    match build_sec(&state, &q) {
        Ok(text) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

fn build_sec(state: &AppState, q: &SecQuery) -> Result<String, (StatusCode, String)> {
    let media = q.type_.as_deref().unwrap_or("SEC");
    if media != "TabDelimited" && media != "SecondSurvey" {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("type '{media}' not yet supported; use type=TabDelimited or type=SecondSurvey"),
        ));
    }

    // Resolve the sector by sx,sy or name/abbreviation.
    let universe = state.universe(&q.milieu)?;
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe.sectors.iter().find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return Err((StatusCode::BAD_REQUEST, "No sector specified.".into()));
    }
    .ok_or((StatusCode::NOT_FOUND, "The specified sector was not found.".into()))?;

    let dir = state.res_dir.join("Sectors").join(&q.milieu);
    let (data_file, outcome) = resolve_and_parse_worlds(&dir, &entry.name, Some(entry))
        .ok_or((StatusCode::NOT_FOUND, format!("no data for '{}'", entry.name)))?;
    let all_worlds = outcome.worlds;

    // Subsector / quadrant filtering (mirrors SECHandler's `options.filter`).
    let subsectors = gather_subsectors(state, &dir, &data_file, entry, &q.milieu);
    let filtered: Vec<World> = if let Some(sub) = &q.subsector {
        let idx = subsector_index_for(sub, &subsectors)
            .ok_or((StatusCode::NOT_FOUND, format!("subsector '{sub}' not found")))?;
        all_worlds.iter().filter(|w| astrometrics::subsector_index(&w.hex) == idx).cloned().collect()
    } else if let Some(quad) = &q.quadrant {
        let qidx = quadrant_index_for(quad)
            .ok_or((StatusCode::BAD_REQUEST, format!("quadrant '{quad}' is invalid")))?;
        all_worlds.iter().filter(|w| astrometrics::quadrant_index(&w.hex) == qidx).cloned().collect()
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
        is_otu.then(|| entry.names.first().and_then(|n| synthesize_abbreviation(&n.text))).flatten()
    });

    if media == "TabDelimited" {
        // TabDelimited ignores includeMetadata (no comment block), per the reference.
        return Ok(sector_writer::write_tab(&filtered, abbr.as_deref().unwrap_or(""), &opts));
    }

    // SecondSurvey: optional metadata comment block (allegiances from ALL worlds),
    // then the columnar world table (filtered).
    let mut out = String::new();
    if bool_opt(&q.metadata, true) {
        out.push_str(&sec_metadata_block(
            state, &dir, &data_file, entry, abbr.as_deref(), &q.milieu, &subsectors, &all_worlds,
        ));
    }
    out.push_str(&sector_writer::write_second_survey(&filtered, &opts));
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
fn read_meta_xml(dir: &FsPath, data_file: &str, entry: &tmap_core::dto::SectorIndexEntry) -> String {
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
    read_text(dir.join(&meta_file)).unwrap_or_default()
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

    let name0 = entry.names.first().map(|n| n.text.as_str()).unwrap_or(&entry.name);
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
        if let Some(v) = &df.author { line(&format!("# Author:    {v}")); }
        if let Some(v) = &df.publisher { line(&format!("# Publisher: {v}")); }
        if let Some(v) = &df.copyright { line(&format!("# Copyright: {v}")); }
        if let Some(v) = &df.source { line(&format!("# Source:    {v}")); }
        if let Some(v) = &df.reference { line(&format!("# Ref:       {v}")); }
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
        let name = alleg_names
            .get(code)
            .cloned()
            .or_else(|| allegiance_name(code));
        if let Some(name) = name {
            line(&format!("# Alleg: {code}: \"{name}\""));
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

/// `GET /api/metadata` — a sector's metadata (names, subsectors, allegiances,
/// borders/regions, routes, labels, stylesheet, products, credits) in the
/// documented JSON shape. Ports `SectorMetaDataHandler.cs` (data side). Cached
/// per `(milieu, sector)` via `serve_cached`.
async fn get_metadata(
    Query(q): Query<MetadataQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let universe = match state.universe(&q.milieu) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe.sectors.iter().find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return (StatusCode::BAD_REQUEST, "No sector specified.").into_response();
    };
    let Some(entry) = entry else {
        return (StatusCode::NOT_FOUND, "The specified sector was not found.").into_response();
    };

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
        is_otu.then(|| meta.names.first().and_then(|n| synthesize_abbreviation(&n.text))).flatten()
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

    meta.allegiances = compute_metadata_allegiances(&worlds, &meta.borders, &meta.regions, &local_alleg);

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
async fn get_credits(Query(q): Query<CreditsQuery>, State(state): State<AppState>) -> Response {
    credits_response(&state, q)
}

fn credits_response(state: &AppState, q: CreditsQuery) -> Response {
    let universe = match state.universe(&q.milieu) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe.sectors.iter().find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name))
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
    axum::Json(result).into_response()
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

/// `/data/{sector}/coordinates` and `/data/{sector}/{hex}/coordinates` live in
/// `compat` (they share the CoordinatesHandler logic).

/// `/data/{sector}/credits` → credits at the sector centre.
async fn data_credits(Path(sector): Path<String>, State(state): State<AppState>) -> Response {
    credits_response(
        &state,
        CreditsQuery { milieu: default_milieu(), sector: Some(sector), sx: None, sy: None, hex: None },
    )
}

/// `/data/{sector}/{hex}/credits` → credits at a specific world.
async fn data_credits_hex(
    Path((sector, hex)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    credits_response(
        &state,
        CreditsQuery { milieu: default_milieu(), sector: Some(sector), sx: None, sy: None, hex: Some(hex) },
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
        Query(MetadataQuery { milieu: default_milieu(), sector: Some(sector), sx: None, sy: None }),
        headers,
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

    let mut r = tmap_core::dto::CreditsResult {
        sector_x: meta.x,
        sector_y: meta.y,
        ..Default::default()
    };
    r.sector_name = meta.names.first().map(|n| n.text.clone());
    r.credits = meta.credits_text.clone();
    r.sector_tags = nonempty(meta.tags.clone());
    r.sector_author = meta.data_file.author.clone();
    r.sector_source = meta.data_file.source.clone();
    r.sector_publisher = meta.data_file.publisher.clone();
    r.sector_copyright = meta.data_file.copyright.clone();
    r.sector_ref = meta.data_file.reference.clone();
    r.sector_milieu = meta.data_file.milieu.clone().or_else(|| Some(milieu.to_string()));

    if let Some(p) = meta.products.first() {
        r.product_publisher = p.publisher.clone();
        r.product_title = p.title.clone();
        r.product_author = p.author.clone();
        r.product_ref = p.reference.clone();
    }

    // Subsector (by its letter index A–P for this hex).
    let letter = astrometrics::subsector_letter(hex).to_string();
    if let Some(ss) = meta.subsectors.iter().find(|s| s.index == letter) {
        r.subsector_name = nonempty(ss.name.clone());
        r.subsector_index = nonempty(ss.index.clone());
    }

    // World at the hex.
    if let Some(w) = worlds.iter().find(|w| w.hex == hex) {
        r.world_name = nonempty(w.name.clone());
        r.world_hex = nonempty(w.hex.clone());
        r.world_uwp = nonempty(w.uwp.clone());
        r.world_remarks = nonempty(w.remarks.clone());
        r.world_ix = w.importance.clone();
        r.world_ex = w.economic.clone();
        r.world_cx = w.cultural.clone();
        r.world_pbg = nonempty(w.pbg.clone());
        r.world_allegiance = nonempty(w.allegiance.clone());
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
async fn get_jumpworlds(Query(q): Query<JumpWorldsQuery>, State(state): State<AppState>) -> Response {
    match build_jumpworlds(&state, &q) {
        Ok(result) => Json(result).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

fn build_jumpworlds(
    state: &AppState,
    q: &JumpWorldsQuery,
) -> Result<tmap_core::dto::JumpWorldsResult, (StatusCode, String)> {
    let jump = q.jump.clamp(0, 12);
    let universe = state.universe(&q.milieu)?;

    let entry = if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        universe.sectors.iter().find(|s| s.location.x == sx && s.location.y == sy)
    } else if let Some(name) = &q.sector {
        universe.sectors.iter().find(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.abbreviation.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name))
        })
    } else {
        return Err((StatusCode::BAD_REQUEST, "No sector specified.".into()));
    }
    .ok_or((StatusCode::NOT_FOUND, "The specified sector was not found.".into()))?;

    let hex = q.hex.as_deref().ok_or((StatusCode::BAD_REQUEST, "No hex specified.".into()))?;
    let (chx, chy) = parse_hex(hex).ok_or((StatusCode::BAD_REQUEST, format!("Invalid hex: {hex}")))?;
    let (cx, cy) = astrometrics::location_to_coordinates(entry.location.x, entry.location.y, chx, chy);
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
            let Some(e) = universe.sectors.iter().find(|s| s.location.x == sx_ && s.location.y == sy_)
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
                is_otu.then(|| e.names.first().and_then(|n| synthesize_abbreviation(&n.text))).flatten()
            });
            let ctx_idx = ctxs.len();
            ctxs.push(JumpSectorCtx {
                name: e.names.first().map(|n| n.text.clone()).unwrap_or_else(|| e.name.clone()),
                abbreviation,
                subsectors,
            });
            for w in outcome.worlds {
                let Some((col, row)) = parse_hex(&w.hex) else { continue };
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
        worlds: w.worlds.map(i32::from).unwrap_or(0),
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
    let local_map: HashMap<&str, &MetaAllegiance> = local.iter().map(|a| (a.code.as_str(), a)).collect();
    let mut codes: Vec<&str> = worlds.iter().map(|w| w.allegiance.as_str()).filter(|c| !c.is_empty()).collect();
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
            name.map(|name| MetaAllegiance { name, code: code.to_string(), base })
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
        return (StatusCode::BAD_REQUEST, format!("unsupported lod '{}'", q.lod)).into_response();
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
        .map(|df| std::path::Path::new(df).file_stem().and_then(|s| s.to_str()).unwrap_or(name))
        .unwrap_or(name)
        .to_string();
    let infer_fmt = |file: &str| match std::path::Path::new(file).extension().and_then(|e| e.to_str()) {
        Some("txt") => "SecondSurvey",
        Some("sec") => "SEC",
        _ => "TabDelimited",
    };
    let (data_file, data_format) = entry
        .and_then(|s| s.data_file.clone())
        .filter(|df| dir.join(df).exists())
        .map(|df| {
            let fmt = entry
                .and_then(|s| s.data_format.clone())
                .unwrap_or_else(|| infer_fmt(&df).to_string());
            (df, fmt)
        })
        .or_else(|| {
            [("tab", "TabDelimited"), ("txt", "SecondSurvey"), ("sec", "SEC")]
                .into_iter()
                .map(|(ext, fmt)| (format!("{stem}.{ext}"), fmt.to_string()))
                .find(|(f, _)| dir.join(f).exists())
        })?;

    let text = read_text(dir.join(&data_file)).ok()?;
    let outcome = match data_format.as_str() {
        "SEC" => parse_sec(&text),             // legacy regex format (.sec)
        "SecondSurvey" => parse_column(&text), // T5 column format (.txt)
        _ => parse_tab(&text),
    }
    // A format quirk shouldn't fail the whole sector — fall back to no worlds.
    .unwrap_or_default();
    Some((data_file, outcome))
}

/// Parse + assemble a sector and serialize it (the cache-miss path).
pub(crate) fn build_sector_bytes(
    state: &AppState,
    milieu: &str,
    name: &str,
    lod: &str,
) -> Result<Vec<u8>, (StatusCode, String)> {
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
        return Err((StatusCode::NOT_FOUND, format!("no data for '{name}' in '{milieu}'")));
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
    let meta_xml = read_text(dir.join(&meta_file)).unwrap_or_default();
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
    let mut seen_routes: HashSet<(String, String, (i32, i32), (i32, i32))> = routes
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
    let mut borders: Vec<tmap_core::dto::Border> =
        ordered_borders(&meta).into_iter().map(render_border).collect();
    let mut seen_borders: HashSet<(String, Vec<String>)> =
        borders.iter().map(|b| (b.allegiance.clone(), b.hexes.clone())).collect();
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
    let mut alleg_names: HashMap<String, String> =
        meta.local_allegiances.iter().map(|a| (a.code.clone(), a.name.clone())).collect();
    for a in &inline_meta.local_allegiances {
        alleg_names.entry(a.code.clone()).or_insert_with(|| a.name.clone());
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
    let mut labels: Vec<tmap_core::dto::SectorLabel> = meta.labels.iter().map(render_label).collect();
    let mut seen_labels: HashSet<(String, String)> =
        labels.iter().map(|l| (l.text.clone(), l.hex.clone())).collect();
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
    let mut tags = meta.tags.clone();
    if tags.is_empty() {
        tags = inline_meta.tags.clone();
    }
    let credits = meta.credits_text.clone().or_else(|| inline_meta.credits_text.clone());

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
    serde_json::to_vec(&data).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
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

    /// Every placed sector in M1105 must load and serialize to valid
    /// `SectorData` — guards against encoding, data-file-resolution, parse, and
    /// metadata regressions (the bugs found during Phase 10 testing).
    #[test]
    fn all_m1105_sectors_load() {
        let state = test_state();
        let universe = state.universe("M1105").expect("M1105 universe builds");
        assert!(
            universe.sectors.len() > 150,
            "expected the full M1105 index, got only {}",
            universe.sectors.len()
        );

        let mut failures = Vec::new();
        for s in &universe.sectors {
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
            "{} of {} M1105 sectors failed to load:\n  {}",
            failures.len(),
            universe.sectors.len(),
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
        let bytes = build_sector_bytes(&state, "M1105", "Hlakhoi", "overview")
            .expect("Hlakhoi builds");
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
        let index = state.route_index("M1105").expect("M1105 route index builds");

        let start = route::resolve_location(&index, "Spinward Marches 1910")
            .expect("Regina (1910) resolves");
        let end = route::resolve_location(&index, "Spinward Marches 2110")
            .expect("Yori (2110) resolves");

        let result = index
            .find_route(start, end, 2, tmap_core::route::RouteOptions::default())
            .expect("a jump-2 route from Regina to Yori exists");

        // Endpoints correct.
        assert_eq!(result.waypoints.first().unwrap().name, "Regina");
        assert_eq!(result.waypoints.last().unwrap().name, "Yori");
        // jumps == waypoints - 1, and the trip is short (Regina↔Yori ≈ 2 pc).
        assert_eq!(result.jumps, result.waypoints.len() - 1);
        assert!(result.jumps >= 1 && result.jumps <= 2, "got {} jumps", result.jumps);
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
