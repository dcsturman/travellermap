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
    astrometrics::{parse_hex, Coord},
    dto::{Overlays, SearchResults, SectorData, SectorInfo, Universe, VectorObject, World, WorldLabel},
    parse::{
        border_region, parse_column, parse_milieu_index, parse_sec, parse_tab, parse_vector_object,
        parse_world_labels, sector_border_styles, sector_borders, sector_index_entry, sector_routes,
        sector_subsectors,
    },
};
use tower_http::cors::CorsLayer;

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
    res_dir: PathBuf,
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
    fn universe(&self, milieu: &str) -> Result<Arc<Universe>, (StatusCode, String)> {
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
}

/// Read a file as text, tolerating non-UTF-8 sector data. Tries UTF-8, then
/// falls back to Latin-1 (every byte → a code point; matches the reference,
/// which reads CP1252) so legacy `.sec` files don't fail.
fn read_text(path: impl AsRef<FsPath>) -> std::io::Result<String> {
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

#[tokio::main]
async fn main() {
    // `res/` lives at the workspace root; override with TMAP_RES_DIR if needed.
    let res_dir = std::env::var("TMAP_RES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("res"));
    let state = AppState {
        res_dir,
        universe_cache: Arc::new(Mutex::new(HashMap::new())),
        overlays: Arc::new(OnceLock::new()),
        search_cache: Arc::new(Mutex::new(HashMap::new())),
        response_cache: Arc::new(Mutex::new(HashMap::new())),
        region_cache: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/universe", get(get_universe))
        .route("/api/overlays", get(get_overlays))
        .route("/api/search", get(get_search))
        .route("/api/sector/{milieu}/{name}", get(get_sector))
        .route("/api/res/{*path}", get(get_res))
        .route("/api/admin/flush", post(flush_cache))
        // Permissive CORS is a dev convenience (Trunk serves the wasm app from
        // a different origin). Tighten before any real deployment.
        .layer(CorsLayer::permissive())
        .with_state(state);

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

/// `GET /api/universe?milieu=M1105` — the sector index for navigation.
async fn get_universe(
    Query(q): Query<UniverseQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let key = format!("universe/{}", q.milieu);
    serve_cached(&state.response_cache, &key, &headers, || {
        let u = state.universe(&q.milieu)?;
        serde_json::to_vec(&*u).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
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

/// Parse + assemble a sector and serialize it (the cache-miss path).
fn build_sector_bytes(
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

    // Resolve the data file: the index's DataFile if it exists, else try the
    // stem with each known extension (sectors not in the region list default to
    // `<name>.tab`, but the data may actually be `.txt`/`.sec`).
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
    let chosen = entry
        .and_then(|s| s.data_file.clone())
        .filter(|df| dir.join(df).exists())
        .map(|df| (df.clone(), entry.and_then(|s| s.data_format.clone()).unwrap_or_else(|| infer_fmt(&df).to_string())))
        .or_else(|| {
            [("tab", "TabDelimited"), ("txt", "SecondSurvey"), ("sec", "SEC")]
                .into_iter()
                .map(|(ext, fmt)| (format!("{stem}.{ext}"), fmt.to_string()))
                .find(|(f, _)| dir.join(f).exists())
        });
    let Some((data_file, data_format)) = chosen else {
        return Err((StatusCode::NOT_FOUND, format!("no data for '{name}' in '{milieu}'")));
    };

    let text = read_text(dir.join(&data_file))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let outcome = match data_format.as_str() {
        "SEC" => parse_sec(&text),                // legacy regex format (.sec)
        "SecondSurvey" => parse_column(&text),    // T5 column format (.txt)
        _ => parse_tab(&text),
    }
    // A format quirk shouldn't 500 the whole sector — fall back to no worlds so
    // it still places + draws.
    .unwrap_or_default();

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

    let mut subsectors = sector_subsectors(&meta_xml);
    if subsectors.is_empty() {
        subsectors = sector_subsectors(&inline);
    }

    let mut routes = sector_routes(&meta_xml);
    let mut seen_routes: HashSet<(String, String, (i32, i32), (i32, i32))> = routes
        .iter()
        .map(|r| (r.start.clone(), r.end.clone(), r.start_offset, r.end_offset))
        .collect();
    for r in sector_routes(&inline) {
        if seen_routes.insert((r.start.clone(), r.end.clone(), r.start_offset, r.end_offset)) {
            routes.push(r);
        }
    }

    let mut borders = sector_borders(&meta_xml);
    let mut seen_borders: HashSet<(String, Vec<String>)> =
        borders.iter().map(|b| (b.allegiance.clone(), b.hexes.clone())).collect();
    for b in sector_borders(&inline) {
        if seen_borders.insert((b.allegiance.clone(), b.hexes.clone())) {
            borders.push(b);
        }
    }

    let mut border_styles = sector_border_styles(&meta_xml);
    for (k, v) in sector_border_styles(&inline) {
        border_styles.entry(k).or_insert(v);
    }

    for b in &mut borders {
        if let Some(loc) = location {
            b.region = border_region(&b.hexes, loc.x, loc.y);
        }
        if b.color.is_none() {
            b.color = border_styles.get(&b.allegiance).cloned();
        }
    }

    let worlds = if lod == "overview" {
        outcome.worlds.into_iter().map(project_overview).collect()
    } else {
        outcome.worlds
    };

    // Review tags + data-source credit (prefer the per-sector xml; the inline
    // region-list block carries Tags too, so fall back to it for tags).
    let mut tags = tmap_core::parse::sector_tags(&meta_xml);
    if tags.is_empty() {
        tags = tmap_core::parse::sector_tags(&inline);
    }
    let credits = tmap_core::parse::sector_credits(&meta_xml)
        .or_else(|| tmap_core::parse::sector_credits(&inline));

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

    fn test_state() -> AppState {
        // `res/` is at the workspace root, two levels up from this crate.
        let res_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../res");
        AppState {
            res_dir,
            universe_cache: Arc::new(Mutex::new(HashMap::new())),
            overlays: Arc::new(OnceLock::new()),
            search_cache: Arc::new(Mutex::new(HashMap::new())),
            response_cache: Arc::new(Mutex::new(HashMap::new())),
            region_cache: Arc::new(Mutex::new(HashMap::new())),
        }
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
}
