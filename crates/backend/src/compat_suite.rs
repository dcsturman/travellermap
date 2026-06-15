//! End-to-end **public-API compatibility** suite (test-driven).
//!
//! Each test drives the *real* axum router in-process (via `tower::oneshot`, the
//! exact routing + handlers we ship) and checks our output against the
//! documented reference shape. Golden fixtures captured from the live
//! travellermap.com API live in `crates/backend/tests/refs/` and are the parity
//! oracle. JSON is compared as `serde_json::Value` (order- and
//! slash-escaping-insensitive); text formats compared verbatim.
//!
//! **TDD convention:** endpoints not yet implemented (or not yet in the public
//! shape) have their test marked `#[ignore = "…"]` with the target assertion
//! already written — so "implement it" = "delete the ignore and it goes green."
//!
//! Progress scoreboard (active vs. pending):
//! ```text
//! cargo test -p tmap-backend compat_suite                  # active (must pass)
//! cargo test -p tmap-backend compat_suite -- --include-ignored   # full target
//! cargo test -p tmap-backend compat_suite -- --ignored --list    # what's left
//! ```

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header::CONTENT_TYPE, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::tests::test_state;
use crate::build_router;

// --- harness -------------------------------------------------------------

/// One GET against a fresh router. Returns `(status, content_type, body)`.
/// Paths must be URL-encoded (spaces as `%20`) — they go straight into the URI.
async fn get(path: &str) -> (StatusCode, String, String) {
    get_with(path, &[]).await
}

async fn get_with(path: &str, headers: &[(&str, &str)]) -> (StatusCode, String, String) {
    let mut rb = Request::builder().method("GET").uri(path);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let resp = build_router(test_state())
        .oneshot(rb.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, ct, String::from_utf8_lossy(&bytes).to_string())
}

/// Read a golden fixture (`tests/refs/<name>`).
fn golden(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/refs")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {name}: {e}"))
}

fn jv(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("parse json ({e}):\n{s}"))
}

/// Assert a JSON body equals a golden fixture, structurally (ignores key order
/// and the reference's cosmetic `\/` slash-escaping — both parse identically).
fn assert_json_matches(body: &str, golden_name: &str) {
    assert_eq!(jv(body), jv(&golden(golden_name)), "vs {golden_name}");
}

/// Assert a JSON *array* body equals a golden fixture as a **set**, keyed by
/// `key`. Used for the T5SS code tables, where the row content must match the
/// reference exactly but the array order is incidental (the reference sorts with
/// .NET's culture-sensitive collation; we sort ordinal).
fn assert_json_set_matches(body: &str, golden_name: &str, key: &str) {
    let sort_by_key = |v: Value| -> Vec<Value> {
        let mut a = v.as_array().expect("array").clone();
        a.sort_by(|x, y| x[key].as_str().unwrap_or("").cmp(y[key].as_str().unwrap_or("")));
        a
    };
    assert_eq!(
        sort_by_key(jv(body)),
        sort_by_key(jv(&golden(golden_name))),
        "vs {golden_name} (as a set keyed by {key})"
    );
}

// ========================================================================
// IMPLEMENTED — active tests, must stay green.
// ========================================================================

// --- Coordinates ---------------------------------------------------------

#[tokio::test]
async fn coordinates_sector_hex() {
    let (status, ct, body) = get("/api/coordinates?sector=Spinward%20Marches&hex=1910").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    assert_json_matches(&body, "coordinates_sector_hex.json");
}

#[tokio::test]
async fn coordinates_world_space_xy() {
    let (status, _, body) = get("/api/coordinates?x=-110&y=-70").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_xy.json");
}

#[tokio::test]
async fn coordinates_subsector() {
    let (status, _, body) = get("/api/coordinates?sector=Spinward%20Marches&subsector=C").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_subsector.json");
}

#[tokio::test]
async fn coordinates_grid_sx_sy() {
    // sx/sy/hx/hy form resolves the same world as the sector+hex form.
    let (status, _, body) = get("/api/coordinates?sx=-4&sy=-1&hx=19&hy=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_sector_hex.json");
}

#[tokio::test]
async fn coordinates_abbreviation_resolves() {
    // T5SS abbreviation resolves identically to the full sector name.
    let (status, _, body) = get("/api/coordinates?sector=Spin&hex=1910").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_sector_hex.json");
}

#[tokio::test]
async fn coordinates_errors() {
    let (status, ..) = get("/api/coordinates").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, ..) = get("/api/coordinates?sector=Nonesuch").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- Milieux -------------------------------------------------------------

#[tokio::test]
async fn milieux() {
    let (status, ct, body) = get("/api/milieux").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    assert_json_matches(&body, "milieux.json");
}

// --- T5SS code tables ----------------------------------------------------

#[tokio::test]
async fn t5ss_allegiances() {
    let (status, _, body) = get("/t5ss/allegiances").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_set_matches(&body, "allegiances.json", "Code");
}

#[tokio::test]
async fn t5ss_sophonts() {
    let (status, _, body) = get("/t5ss/sophonts").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_set_matches(&body, "sophonts.json", "Code");
}

// --- Universe (shape unified; completeness still pending) ----------------

fn sectors_by_xy(v: &Value) -> HashMap<(i64, i64), &Value> {
    v["Sectors"]
        .as_array()
        .expect("Sectors array")
        .iter()
        .map(|s| ((s["X"].as_i64().unwrap(), s["Y"].as_i64().unwrap()), s))
        .collect()
}

#[tokio::test]
async fn universe_envelope_and_known_sectors_match() {
    let (status, ct, body) = get("/api/universe?milieu=M1105").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    let ours = jv(&body);
    assert!(ours.get("Sectors").is_some(), "public envelope {{\"Sectors\":[…]}}");

    // Well-known sectors must be byte-identical to the reference (full shape:
    // X/Y/Milieu/Abbreviation/Tags/Names incl. localized entries).
    let theirs = jv(&golden("universe_m1105.json"));
    let ours_map = sectors_by_xy(&ours);
    let theirs_map = sectors_by_xy(&theirs);
    for xy in [(-4, -1), (0, -21)] {
        let o = ours_map.get(&xy).unwrap_or_else(|| panic!("we omit sector at {xy:?}"));
        let t = theirs_map.get(&xy).unwrap_or_else(|| panic!("ref omits sector at {xy:?}"));
        assert_eq!(o, t, "sector at {xy:?} differs from reference");
    }
}

// --- JSONP ---------------------------------------------------------------

#[tokio::test]
async fn jsonp_wraps_payload() {
    let (status, ct, body) = get("/api/milieux?jsonp=cb").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("javascript"), "ct={ct}");
    assert!(body.starts_with("cb(") && body.ends_with(");"), "not wrapped: {body}");
    // The wrapped payload is exactly the JSON body.
    let inner = &body[3..body.len() - 2];
    assert_eq!(jv(inner), jv(&golden("milieux.json")));
}

#[tokio::test]
async fn jsonp_rejects_bad_callback() {
    let (status, ..) = get("/api/milieux?jsonp=not.valid").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ========================================================================
// PENDING — `#[ignore]`d until implemented. Each carries the target
// assertion; remove the ignore when the endpoint lands.  See PORT_API_COMPAT.md.
// ========================================================================

// --- Universe: full sector-set completeness ------------------------------

#[tokio::test]
#[ignore = "universe completeness: include positioned-but-dataless sectors (~1021 vs ~190)"]
async fn universe_full_sector_set() {
    let (_, _, body) = get("/api/universe?milieu=M1105").await;
    let ours = sectors_by_xy(&jv(&body)).len();
    let theirs = sectors_by_xy(&jv(&golden("universe_m1105.json"))).len();
    assert_eq!(ours, theirs, "sector count parity");
}

#[tokio::test]
#[ignore = "universe completeness: every returned sector must match the reference at its grid position"]
async fn universe_all_returned_sectors_match() {
    let (_, _, body) = get("/api/universe?milieu=M1105").await;
    let ours = jv(&body);
    let theirs = jv(&golden("universe_m1105.json"));
    let ours_map = sectors_by_xy(&ours);
    let theirs_map = sectors_by_xy(&theirs);
    for (xy, o) in &ours_map {
        let t = theirs_map.get(xy).unwrap_or_else(|| panic!("ref omits {xy:?}"));
        assert_eq!(o, t, "sector at {xy:?} differs");
    }
}

// --- Search: documented Results.Items envelope ---------------------------

#[tokio::test]
#[ignore = "search: emit public {Results:{Count,Items:[{World|Sector|Subsector|Label}]}} (needs Tantivy)"]
async fn search_public_envelope() {
    let (status, _, body) = get("/api/search?q=Regina").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "search_regina.json");
}

// --- SEC / tab text output -----------------------------------------------

#[tokio::test]
#[ignore = "sec: emit TabDelimited text via SectorWriter"]
async fn sec_tab_delimited() {
    let (status, ct, body) = get("/api/sec?sector=Spinward%20Marches&subsector=A&type=TabDelimited").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/plain"), "ct={ct}");
    assert_eq!(body, golden("sec_sm_subsectorA.tab"));
}

#[tokio::test]
#[ignore = "sec: emit SecondSurvey text via SectorWriter"]
async fn sec_second_survey() {
    let (status, _, body) = get("/api/sec?sector=Spinward%20Marches&subsector=A&type=SecondSurvey").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, golden("sec_sm_subsectorA.sec"));
}

// --- Metadata ------------------------------------------------------------

#[tokio::test]
#[ignore = "metadata: standalone full sector metadata JSON (needs fuller parse + serializer)"]
async fn metadata_json() {
    let (status, _, body) = get("/api/metadata?sector=Spinward%20Marches").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "metadata_sm.json");
}

// --- MSEC ----------------------------------------------------------------

#[tokio::test]
#[ignore = "msec: emit MSEC metadata text via MSECWriter"]
async fn msec_text() {
    let (status, _, body) = get("/api/msec?sector=Spinward%20Marches").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, golden("msec_sm.msec"));
}

// --- Credits -------------------------------------------------------------

#[tokio::test]
#[ignore = "credits: per-location credits/author/source (needs metadata credit fields)"]
async fn credits_json() {
    let (status, _, body) = get("/api/credits?sector=Spinward%20Marches&hex=1910").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "credits_sm_1910.json");
}

// --- JumpWorlds ----------------------------------------------------------

#[tokio::test]
#[ignore = "jumpworlds: worlds within N parsecs as {Worlds:[…]} (needs cross-sector spatial index)"]
async fn jumpworlds_json() {
    let (status, _, body) = get("/api/jumpworlds?sector=Spinward%20Marches&hex=1910&jump=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "jumpworlds_sm_1910_j2.json");
}

// --- Route: documented bare-array public shape ---------------------------
// Our /api/route works but emits a private {waypoints,jumps,parsecs} object;
// the public API returns a bare array of stops. This asserts the public shape.

#[tokio::test]
#[ignore = "route: emit the public bare-array stop shape (currently {waypoints,…})"]
async fn route_public_shape() {
    let (status, _, body) = get("/api/route?start=Spinward%20Marches%201910&end=Spinward%20Marches%202410&jump=2").await;
    assert_eq!(status, StatusCode::OK);
    let v = jv(&body);
    let stops = v.as_array().expect("public route is a bare array of stops");
    assert_eq!(stops.first().unwrap()["Name"], "Regina");
    assert_eq!(stops.last().unwrap()["Name"], "Inthe");
    // Public per-stop keys.
    for k in ["Sector", "SectorX", "SectorY", "Name", "Hex", "HexX", "HexY", "UWP", "PBG", "Zone", "AllegianceName"] {
        assert!(stops[0].get(k).is_some(), "stop missing {k}");
    }
}

// --- Semantic /data/... URL aliases --------------------------------------

#[tokio::test]
#[ignore = "/data alias: /data/{sector}/{hex}/coordinates"]
async fn data_alias_coordinates() {
    let (status, _, body) = get("/data/Spinward%20Marches/1910/coordinates").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_sector_hex.json");
}

#[tokio::test]
#[ignore = "/data alias: /data/{sector}/tab"]
async fn data_alias_sec_tab() {
    let (status, ..) = get("/data/Spinward%20Marches/tab").await;
    assert_eq!(status, StatusCode::OK);
}

// --- Content negotiation: Accept: text/xml -------------------------------

#[tokio::test]
#[ignore = "content negotiation: Accept: text/xml on data endpoints"]
async fn xml_content_negotiation() {
    let (status, ct, _) = get_with(
        "/api/coordinates?sector=Spinward%20Marches&hex=1910",
        &[("accept", "text/xml")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("xml"), "ct={ct}");
}
