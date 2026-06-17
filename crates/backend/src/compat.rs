//! Drop-in compatibility layer for the documented public Traveller Map data API
//! (<https://travellermap.com/doc/api>).
//!
//! These endpoints mirror the reference `server/api/` handlers' URL shapes and
//! **PascalCase** JSON envelopes so existing third-party clients and tools work
//! unchanged. They are purely *additive*: our own Leptos client keeps using the
//! private snake_case contract (`/api/sector/...`, `/api/universe`, …), which is
//! shaped for client-side rendering. See `PORT_API_COMPAT.md`.
//!
//! Implemented here — the "cheap" reshapes (pure functions over data we already
//! hold or tiny static tables), none of which collide with the private contract:
//!
//! | URL | Envelope |
//! |---|---|
//! | `GET /api/coordinates` | `{"sx","sy","hx","hy","x","y"}` |
//! | `GET /api/milieux` | `[{"Code","IsDefault"}]` |
//! | `GET /t5ss/allegiances` | `[{"Code","LegacyCode","Name","Location"}]` |
//! | `GET /t5ss/sophonts` | `[{"Code","Name","Location"}]` |
//!
//! All honor `&jsonp=<callback>` (JSONP). Content negotiation for
//! `Accept: text/xml` is a deliberate follow-up (the documented default is JSON,
//! which is what these emit).
//!
//! The reshape-the-private-contract endpoints (`universe`, `search`, `sec`,
//! `metadata`, `jumpworlds`, `route`) are *not* here — they either collide with
//! the private contract or need new index/writer work; see `PORT_API_COMPAT.md`.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use tmap_core::astrometrics::{self, SUBSECTOR_HEIGHT, SUBSECTOR_WIDTH};

use crate::{read_text, AppState};

/// A `jsonp` query parameter, shared by every compat endpoint.
#[derive(Debug, Deserialize, Default)]
pub struct Jsonp {
    pub jsonp: Option<String>,
}

/// JSON identifier guard for the JSONP callback (mirrors the reference
/// `IsSimpleJSIdentifier`) so the wrapper can't inject arbitrary script.
fn is_simple_js_identifier(s: &str) -> bool {
    let mut bytes = s.bytes();
    match bytes.next() {
        Some(b) if b == b'_' || b.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|b| b == b'_' || b.is_ascii_alphanumeric())
}

/// Serialize `value` as JSON, wrapping it as `callback(...);` when a valid
/// `jsonp` parameter is present (served as `text/javascript`). Serializes the
/// value directly (no `serde_json::Value` intermediate) so struct field order is
/// preserved, matching the reference byte-for-byte.
pub(crate) fn respond<T: Serialize>(value: &T, jsonp: &Option<String>) -> Response {
    let json = match serde_json::to_string(value) {
        Ok(j) => j,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match jsonp {
        Some(cb) if is_simple_js_identifier(cb) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
            .body(Body::from(format!("{cb}({json});")))
            .unwrap(),
        Some(_) => (
            StatusCode::BAD_REQUEST,
            "the jsonp parameter must be a simple script identifier".to_string(),
        )
            .into_response(),
        None => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap(),
    }
}

/// Whether the client prefers XML (the reference's default content type for the
/// data endpoints) — `Accept: text/xml` / `application/xml`.
pub fn wants_xml(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("xml"))
}

/// Serve `value` honoring content negotiation: `jsonp` callback (JS) wins, then
/// `Accept: …xml` (the `xml` closure builds the body), else JSON. The XML body
/// is produced lazily so callers only pay for it when XML is requested.
pub(crate) fn respond_negotiated<T: Serialize>(
    value: &T,
    jsonp: &Option<String>,
    accept_xml: bool,
    xml: impl FnOnce() -> String,
) -> Response {
    if jsonp.is_none() && accept_xml {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/xml; charset=utf-8")
            .body(Body::from(xml()))
            .unwrap();
    }
    respond(value, jsonp)
}

// ---------------------------------------------------------------------------
// Coordinates — name/coord → {sx,sy,hx,hy,x,y}
// ---------------------------------------------------------------------------

fn default_milieu() -> String {
    "M1105".to_string()
}

#[derive(Debug, Deserialize)]
pub struct CoordinatesQuery {
    #[serde(default = "default_milieu")]
    pub milieu: String,
    /// Sector by display name or T5SS abbreviation.
    sector: Option<String>,
    /// 4-digit hex within the sector (e.g. `1910`); defaults to `0` (→ hex 0,0).
    hex: Option<String>,
    /// Subsector letter A–P or name (centre of that subsector).
    subsector: Option<String>,
    // Direct grid form.
    sx: Option<i32>,
    sy: Option<i32>,
    hx: Option<i32>,
    hy: Option<i32>,
    // World-space form.
    x: Option<i32>,
    y: Option<i32>,
}

impl Default for CoordinatesQuery {
    fn default() -> Self {
        Self {
            milieu: default_milieu(),
            sector: None,
            hex: None,
            subsector: None,
            sx: None,
            sy: None,
            hx: None,
            hy: None,
            x: None,
            y: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct CoordinatesResult {
    sx: i32,
    sy: i32,
    hx: i32,
    hy: i32,
    x: i32,
    y: i32,
}

impl CoordinatesResult {
    /// `<Coordinates><sx>…</sx>…</Coordinates>` — the reference
    /// `CoordinatesResult` XML (all integer elements, no escaping needed).
    fn to_xml(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <Coordinates><sx>{}</sx><sy>{}</sy><hx>{}</hx><hy>{}</hy><x>{}</x><y>{}</y></Coordinates>",
            self.sx, self.sy, self.hx, self.hy, self.x, self.y
        )
    }
}

/// `GET /api/coordinates` — resolve a sector name / grid / world-space input to
/// the full `{sx,sy,hx,hy,x,y}` location. Port of `CoordinatesHandler.cs`.
pub async fn get_coordinates(
    Query(q): Query<CoordinatesQuery>,
    Query(jp): Query<Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    coordinates_response(&state, &q, &jp.jsonp, wants_xml(&headers))
}

/// `/data/{sector}/{hex}/coordinates` semantic alias (CoordinatesHandler).
pub async fn data_coordinates_hex(
    axum::extract::Path((sector, hex)): axum::extract::Path<(String, String)>,
    Query(jp): Query<Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let q = CoordinatesQuery {
        sector: Some(sector),
        hex: Some(hex),
        ..CoordinatesQuery::default()
    };
    coordinates_response(&state, &q, &jp.jsonp, wants_xml(&headers))
}

/// `/data/{sector}/coordinates` semantic alias (sector centre).
pub async fn data_coordinates(
    axum::extract::Path(sector): axum::extract::Path<String>,
    Query(jp): Query<Jsonp>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let q = CoordinatesQuery {
        sector: Some(sector),
        ..CoordinatesQuery::default()
    };
    coordinates_response(&state, &q, &jp.jsonp, wants_xml(&headers))
}

/// Shared body for the coordinates endpoint + its `/data/...` aliases.
fn coordinates_response(
    state: &AppState,
    q: &CoordinatesQuery,
    jsonp: &Option<String>,
    accept_xml: bool,
) -> Response {
    let (sx, sy, hx, hy) = match resolve_location(state, q) {
        Ok(loc) => loc,
        Err((code, msg)) => return (code, msg).into_response(),
    };
    let (x, y) = astrometrics::location_to_coordinates(sx, sy, hx, hy);
    let result = CoordinatesResult { sx, sy, hx, hy, x, y };
    respond_negotiated(&result, jsonp, accept_xml, || result.to_xml())
}

/// Resolve the coordinates query's various input forms to `(sx,sy,hx,hy)`.
fn resolve_location(
    state: &AppState,
    q: &CoordinatesQuery,
) -> Result<(i32, i32, i32, i32), (StatusCode, String)> {
    // 1. Sector by name/abbreviation (+ optional hex or subsector).
    if let Some(sector_name) = &q.sector {
        let universe = state.universe(&q.milieu)?;
        let entry = universe
            .sectors
            .iter()
            .find(|s| {
                s.name.eq_ignore_ascii_case(sector_name)
                    || s.abbreviation
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(sector_name))
            })
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("The specified sector '{sector_name}' was not found."),
                )
            })?;
        let (sx, sy) = (entry.location.x, entry.location.y);

        if let Some(sub) = &q.subsector {
            let index = subsector_index(state, &q.milieu, &entry.name, sub).ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("The specified subsector '{sub}' was not found."),
                )
            })?;
            let index = index as i32;
            let hx = (index % 4) * SUBSECTOR_WIDTH + SUBSECTOR_WIDTH / 2;
            let hy = (index / 4) * SUBSECTOR_HEIGHT + SUBSECTOR_HEIGHT / 2;
            return Ok((sx, sy, hx, hy));
        }

        // hex is a 4-digit "ccrr"; absent → 0 → hex (0,0), matching the reference.
        let hex = q.hex.as_deref().unwrap_or("0").parse::<i32>().unwrap_or(0);
        let (hx, hy) = (hex / 100, hex % 100);
        return Ok((sx, sy, hx, hy));
    }

    // 2. Direct grid: sx,sy (+ optional hx,hy).
    if let (Some(sx), Some(sy)) = (q.sx, q.sy) {
        return Ok((sx, sy, q.hx.unwrap_or(0), q.hy.unwrap_or(0)));
    }

    // 3. World-space: x,y → location.
    if let (Some(x), Some(y)) = (q.x, q.y) {
        return Ok(astrometrics::coordinates_to_location(x, y));
    }

    Err((
        StatusCode::BAD_REQUEST,
        "Must specify either sector name (and optional hex) or sx, sy (and optional \
         hx, hy), or x, y (world-space coordinates)."
            .to_string(),
    ))
}

/// Find a subsector's 0-based index (A=0 … P=15) within a sector, by single
/// letter or by name. Loads the sector's parsed subsector list on demand.
fn subsector_index(state: &AppState, milieu: &str, sector: &str, query: &str) -> Option<usize> {
    // A single letter A–P addresses the subsector directly.
    if query.len() == 1 {
        let c = query.chars().next().unwrap().to_ascii_uppercase();
        if ('A'..='P').contains(&c) {
            return Some((c as u8 - b'A') as usize);
        }
    }
    // Otherwise match a subsector name (case-insensitive) from the metadata.
    let bytes = crate::build_sector_bytes(state, milieu, sector, "overview").ok()?;
    let data: tmap_core::dto::SectorData = serde_json::from_slice(&bytes).ok()?;
    data.info
        .subsectors
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(query))
        .and_then(|s| {
            // Subsector index from its letter ("A"-"P").
            s.index.chars().next().map(|c| (c.to_ascii_uppercase() as u8 - b'A') as usize)
        })
}

// ---------------------------------------------------------------------------
// Milieux — [{Code, IsDefault}]
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct MilieuResult {
    #[serde(rename = "Code")]
    code: String,
    #[serde(rename = "IsDefault")]
    is_default: bool,
}

/// `GET /api/milieux` — the canonical OTU milieu codes, in `milieu.tab` order
/// (M1105 first), each flagged `IsDefault`. Port of `MilieuxCodesHandler`.
pub async fn get_milieux(Query(jp): Query<Jsonp>, State(state): State<AppState>) -> Response {
    let codes = canonical_milieux(&state);
    let list: Vec<MilieuResult> = codes
        .into_iter()
        .map(|code| MilieuResult {
            is_default: code == "M1105",
            code,
        })
        .collect();
    respond(&list, &jp.jsonp)
}

/// Canonical milieu codes (`M<digits>` or `IW`) in `milieu.tab` order, deduped.
/// Matches the reference `GetMilieux()` output exactly (the non-OTU settings —
/// Deepnight, Orion OB1, … — are excluded because they aren't milieu codes).
fn canonical_milieux(state: &AppState) -> Vec<String> {
    let text = read_text(state.res_dir.join("Sectors").join("milieu.tab")).unwrap_or_default();
    let mut seen = Vec::new();
    for line in text.lines().skip(1) {
        let Some(path) = line.split('\t').next() else { continue };
        let Some(dir) = path.split('/').next() else { continue };
        let is_milieu = dir == "IW"
            || (dir.starts_with('M') && dir.len() > 1 && dir[1..].bytes().all(|b| b.is_ascii_digit()));
        if is_milieu && !seen.iter().any(|c| c == dir) {
            seen.push(dir.to_string());
        }
    }
    seen
}

// ---------------------------------------------------------------------------
// T5SS code tables — allegiances / sophonts
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AllegianceCode {
    #[serde(rename = "Code")]
    code: String,
    #[serde(rename = "LegacyCode")]
    legacy_code: String,
    #[serde(rename = "Name")]
    name: String,
    // The reference omits a null `Location` (stock allegiances have none).
    #[serde(rename = "Location", skip_serializing_if = "String::is_empty")]
    location: String,
}

#[derive(Debug, Serialize)]
struct SophontCode {
    #[serde(rename = "Code")]
    code: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Location")]
    location: String,
}

/// Stock allegiances the reference merges on top of the `.tab` file
/// (`SecondSurvey.cs` `s_t5Allegiances` — the "Unofficial/Unreviewed" M1120
/// splinter states + cultural regions). `(Code, LegacyCode, Name)`; these have
/// no `Location`. None overlap the `.tab`, so all are appended.
const STOCK_ALLEGIANCES: &[(&str, &str, &str)] = &[
    ("FdAr", "Fa", "Federation of Arden"),
    ("BoWo", "Bw", "Border Worlds"),
    ("LuIm", "Li", "Lucan's Imperium"),
    ("MaSt", "Ma", "Maragaret's Domain"),
    ("BaCl", "Bc", "Backman Cluster"),
    ("FdDa", "Fd", "Federation of Daibei"),
    ("FdIl", "Fi", "Federation of Ilelish"),
    ("AvCn", "Ac", "Avalar Consulate"),
    ("CoAl", "Ca", "Corsair Alliance"),
    ("StIm", "St", "Strephon's Worlds"),
    ("ZiSi", "Rv", "Restored Vilani Imperium"),
    ("VA16", "V6", "Assemblage of 1116"),
    ("CRVi", "CV", "Vilani Cultural Region"),
    ("CRGe", "CG", "Geonee Cultural Region"),
    ("CRSu", "CS", "Suerrat Cultural Region"),
    ("CRAk", "CA", "Anakudnu Cultural Region"),
];

/// `GET /t5ss/allegiances` — the T5SS allegiance code table
/// (`res/t5ss/allegiance_codes.tab` + [`STOCK_ALLEGIANCES`]), sorted by code.
/// Tab columns: `Code  Legacy  BaseCode  Name  Location`.
pub async fn get_allegiances(Query(jp): Query<Jsonp>, State(state): State<AppState>) -> Response {
    let path = state.res_dir.join("t5ss").join("allegiance_codes.tab");
    let text = read_text(path).unwrap_or_default();
    let mut list: Vec<AllegianceCode> = text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut f = line.split('\t');
            let code = f.next()?.trim();
            let legacy = f.next().unwrap_or("").trim();
            let _base = f.next(); // BaseCode — unused in the public shape.
            let name = f.next().unwrap_or("").trim();
            let location = f.next().unwrap_or("").trim();
            if code.is_empty() {
                return None;
            }
            Some(AllegianceCode {
                code: code.to_string(),
                legacy_code: legacy.to_string(),
                name: name.to_string(),
                location: location.to_string(),
            })
        })
        .collect();
    for &(code, legacy, name) in STOCK_ALLEGIANCES {
        list.push(AllegianceCode {
            code: code.to_string(),
            legacy_code: legacy.to_string(),
            name: name.to_string(),
            location: String::new(),
        });
    }
    list.sort_by(|a, b| a.code.cmp(&b.code));
    respond(&list, &jp.jsonp)
}

/// `GET /t5ss/sophonts` — the T5SS sophont code table
/// (`res/t5ss/sophont_codes.tab`), sorted by code. Columns: `Code  Name  Location`.
pub async fn get_sophonts(Query(jp): Query<Jsonp>, State(state): State<AppState>) -> Response {
    let path = state.res_dir.join("t5ss").join("sophont_codes.tab");
    let text = read_text(path).unwrap_or_default();
    let mut list: Vec<SophontCode> = text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut f = line.split('\t');
            let code = f.next()?.trim();
            let name = f.next().unwrap_or("").trim();
            let location = f.next().unwrap_or("").trim();
            if code.is_empty() {
                return None;
            }
            Some(SophontCode {
                code: code.to_string(),
                name: name.to_string(),
                location: location.to_string(),
            })
        })
        .collect();
    list.sort_by(|a, b| a.code.cmp(&b.code));
    respond(&list, &jp.jsonp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_state;

    fn coords(state: &AppState, q: CoordinatesQuery) -> (i32, i32, i32, i32, i32, i32) {
        let (sx, sy, hx, hy) = resolve_location(state, &q).expect("resolves");
        let (x, y) = astrometrics::location_to_coordinates(sx, sy, hx, hy);
        (sx, sy, hx, hy, x, y)
    }

    fn q() -> CoordinatesQuery {
        CoordinatesQuery {
            milieu: "M1105".into(),
            sector: None,
            hex: None,
            subsector: None,
            sx: None,
            sy: None,
            hx: None,
            hy: None,
            x: None,
            y: None,
        }
    }

    #[test]
    fn coordinates_match_live_api() {
        let state = test_state();
        // sector + hex (the canonical example from the live API).
        assert_eq!(
            coords(&state, CoordinatesQuery { sector: Some("Spinward Marches".into()), hex: Some("1910".into()), ..q() }),
            (-4, -1, 19, 10, -110, -70)
        );
        // T5SS abbreviation resolves the same as the full name.
        assert_eq!(
            coords(&state, CoordinatesQuery { sector: Some("Spin".into()), hex: Some("1910".into()), ..q() }),
            (-4, -1, 19, 10, -110, -70)
        );
        // world-space x,y → location round-trips.
        assert_eq!(
            coords(&state, CoordinatesQuery { x: Some(-110), y: Some(-70), ..q() }),
            (-4, -1, 19, 10, -110, -70)
        );
        // subsector C (centre) — matches live {sx:-4,sy:-1,hx:20,hy:5}.
        assert_eq!(
            coords(&state, CoordinatesQuery { sector: Some("Spinward Marches".into()), subsector: Some("C".into()), ..q() }),
            (-4, -1, 20, 5, -109, -75)
        );
    }

    #[test]
    fn coordinates_errors_are_clean() {
        let state = test_state();
        // No input → 400.
        assert_eq!(resolve_location(&state, &q()).unwrap_err().0, StatusCode::BAD_REQUEST);
        // Unknown sector → 404.
        let bad = CoordinatesQuery { sector: Some("Nonesuch".into()), ..q() };
        assert_eq!(resolve_location(&state, &bad).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn milieux_match_live_order() {
        let state = test_state();
        assert_eq!(
            canonical_milieux(&state),
            ["M1105", "IW", "M0", "M600", "M990", "M1120", "M1201", "M1248", "M1900"]
        );
    }

    #[test]
    fn jsonp_identifier_guard() {
        assert!(is_simple_js_identifier("cb"));
        assert!(is_simple_js_identifier("_x9"));
        assert!(!is_simple_js_identifier("9x"));
        assert!(!is_simple_js_identifier("a.b"));
        assert!(!is_simple_js_identifier(""));
    }
}
