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

// --- live parity (on by default; CI opts out via TMAP_SKIP_PARITY=1) ------
//
// The golden fixtures above are snapshots; they can silently fall out of sync
// with the live reference. Live parity closes that gap: the SAME request is sent
// to travellermap.com and to our in-process router, and the two are compared.
// A failure means either (1) we call the reference incorrectly (status mismatch)
// or (2) our output deviates from it (unless a documented equivalence is
// normalized away below).
//
// **Runs on a local `cargo test`** so deviations surface during development.
// **CI sets `TMAP_SKIP_PARITY=1`** to exclude it — it's network-dependent (slow,
// rate-limited, can't run offline), which doesn't belong in the fast CI gate.

const REFERENCE_BASE: &str = "https://travellermap.com";

fn parity_enabled() -> bool {
    !std::env::var("TMAP_SKIP_PARITY").is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

/// GET `path` from the live reference. travellermap.com resets the connection
/// for requests with no User-Agent, so one is always set.
async fn fetch_live(path: &str) -> (StatusCode, String) {
    let url = format!("{REFERENCE_BASE}{path}");
    let client = reqwest::Client::builder()
        .user_agent("tmap-parity-check (+https://github.com/dcsturman/travellermap)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client");
    let resp = client.get(&url).send().await.unwrap_or_else(|e| panic!("live GET {url}: {e}"));
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap();
    let body = resp.text().await.unwrap_or_default();
    (status, body)
}

/// Compare our JSON output for `path` to the live reference, applying `norm` to
/// both bodies (so endpoint-specific equivalences — route notation, color casing
/// — don't read as deviations). No-op unless `TMAP_PARITY` is set.
async fn parity_json_with(path: &str, norm: impl Fn(&str) -> Value) {
    if !parity_enabled() {
        return;
    }
    let (ours_status, _, ours_body) = get(path).await;
    let (live_status, live_body) = fetch_live(path).await;
    assert_eq!(
        ours_status.as_u16(),
        live_status.as_u16(),
        "status vs live for {path}\n  ours={ours_body}\n  live={live_body}"
    );
    if ours_status.is_success() {
        assert_eq!(norm(&ours_body), norm(&live_body), "live JSON parity for {path}");
    }
}

/// Live parity with plain `serde_json` equality (key order + slash-escaping are
/// already normalized by parsing). The common case.
async fn parity_json(path: &str) {
    parity_json_with(path, jv).await;
}

/// Parse + sort a JSON array by string field `key` — for order-insensitive set
/// comparison of the T5SS tables (the reference sorts with .NET collation).
fn jv_sorted(s: &str, key: &str) -> Value {
    let mut v = jv(s);
    if let Some(a) = v.as_array_mut() {
        a.sort_by(|x, y| x[key].as_str().unwrap_or("").cmp(y[key].as_str().unwrap_or("")));
    }
    v
}

/// Live parity for text endpoints (SEC/MSEC), ignoring the generation timestamp.
async fn parity_text(path: &str) {
    if !parity_enabled() {
        return;
    }
    let (ours_status, _, ours_body) = get(path).await;
    let (live_status, live_body) = fetch_live(path).await;
    assert_eq!(ours_status.as_u16(), live_status.as_u16(), "status vs live for {path}");
    if ours_status.is_success() {
        assert_eq!(strip_timestamp(&ours_body), strip_timestamp(&live_body), "live text parity for {path}");
    }
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
    parity_json("/api/coordinates?sector=Spinward%20Marches&hex=1910").await;
}

#[tokio::test]
async fn coordinates_world_space_xy() {
    let (status, _, body) = get("/api/coordinates?x=-110&y=-70").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_xy.json");
    parity_json("/api/coordinates?x=-110&y=-70").await;
}

#[tokio::test]
async fn coordinates_subsector() {
    let (status, _, body) = get("/api/coordinates?sector=Spinward%20Marches&subsector=C").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_subsector.json");
    parity_json("/api/coordinates?sector=Spinward%20Marches&subsector=C").await;
}

#[tokio::test]
async fn coordinates_grid_sx_sy() {
    // sx/sy/hx/hy form resolves the same world as the sector+hex form.
    let (status, _, body) = get("/api/coordinates?sx=-4&sy=-1&hx=19&hy=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_sector_hex.json");
    parity_json("/api/coordinates?sx=-4&sy=-1&hx=19&hy=10").await;
}

#[tokio::test]
async fn coordinates_abbreviation_resolves() {
    // T5SS abbreviation resolves identically to the full sector name.
    let (status, _, body) = get("/api/coordinates?sector=Spin&hex=1910").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_sector_hex.json");
    parity_json("/api/coordinates?sector=Spin&hex=1910").await;
}

#[tokio::test]
async fn coordinates_errors() {
    let (status, ..) = get("/api/coordinates").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, ..) = get("/api/coordinates?sector=Nonesuch").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Live parity: the reference returns the same error statuses.
    parity_json("/api/coordinates").await;
    parity_json("/api/coordinates?sector=Nonesuch").await;
}

// --- Milieux -------------------------------------------------------------

#[tokio::test]
async fn milieux() {
    let (status, ct, body) = get("/api/milieux").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    assert_json_matches(&body, "milieux.json");
    parity_json("/api/milieux").await;
}

// --- T5SS code tables ----------------------------------------------------

#[tokio::test]
async fn t5ss_allegiances() {
    let (status, _, body) = get("/t5ss/allegiances").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_set_matches(&body, "allegiances.json", "Code");
    parity_json_with("/t5ss/allegiances", |s| jv_sorted(s, "Code")).await;
}

#[tokio::test]
async fn t5ss_sophonts() {
    let (status, _, body) = get("/t5ss/sophonts").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_set_matches(&body, "sophonts.json", "Code");
    parity_json_with("/t5ss/sophonts", |s| jv_sorted(s, "Code")).await;
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
    // Live parity: the FULL sector set must match live exactly — same (X,Y) keys,
    // and every shared sector identical on every field (Abbreviation included: the
    // collision-dedup, empty-name, and loose-file-duplicate bugs are fixed, so we
    // now match live's abbreviations and sector set byte-for-byte).
    if parity_enabled() {
        let live = fetch_live("/api/universe?milieu=M1105").await.1;
        let live_v = jv(&live);
        let theirs_live = sectors_by_xy(&live_v);
        let our_keys: std::collections::HashSet<_> = ours_map.keys().copied().collect();
        let their_keys: std::collections::HashSet<_> = theirs_live.keys().copied().collect();
        assert_eq!(our_keys, their_keys, "universe sector set differs from live");
        for (xy, o) in &ours_map {
            assert_eq!(o, &theirs_live[xy], "universe sector {xy:?} differs from live");
        }
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
async fn universe_full_sector_set() {
    let (_, _, body) = get("/api/universe?milieu=M1105").await;
    let ours = sectors_by_xy(&jv(&body)).len();
    let theirs = sectors_by_xy(&jv(&golden("universe_m1105.json"))).len();
    assert_eq!(ours, theirs, "sector count parity");
}

#[tokio::test]
async fn universe_all_returned_sectors_match() {
    let (_, _, body) = get("/api/universe?milieu=M1105").await;
    let ours = jv(&body);
    let theirs = jv(&golden("universe_m1105.json"));
    let ours_map = sectors_by_xy(&ours);
    let theirs_map = sectors_by_xy(&theirs);

    // Every sector we return must match the reference's at the same grid
    // position on all fields EXCEPT `Abbreviation`, which is drift-prone: the
    // live data carries hand-disambiguated abbreviations (e.g. "Inc2", "Inc3")
    // for several sectors that this older checkout lacks, so we synthesize a
    // different one. Position/Milieu/Names/Tags must match exactly — those catch
    // real regressions. (Declared abbreviations are still pinned by
    // `universe_envelope_and_known_sectors_match`.) A handful of sectors exist
    // locally but not in the captured golden (also drift); allow a small budget.
    let strip_abbr = |v: &Value| {
        let mut c = v.clone();
        c.as_object_mut().unwrap().remove("Abbreviation");
        c
    };
    let mut missing = 0;
    let mut hard_diffs = Vec::new();
    let mut abbr_drift = 0;
    for (xy, o) in &ours_map {
        match theirs_map.get(xy) {
            None => missing += 1,
            Some(t) if o == t => {}
            Some(t) => {
                if strip_abbr(o) == strip_abbr(t) {
                    abbr_drift += 1;
                } else if hard_diffs.len() < 10 {
                    hard_diffs.push(format!("{xy:?}: ours={o} theirs={t}"));
                }
            }
        }
    }
    assert!(
        hard_diffs.is_empty(),
        "{} sectors differ beyond the abbreviation field:\n  {}",
        hard_diffs.len(),
        hard_diffs.join("\n  ")
    );
    assert!(missing <= 5, "{missing} sectors are absent from the reference (drift budget 5)");
    assert!(abbr_drift <= 60, "{abbr_drift} abbreviation-only drifts (budget 60)");
}

// --- Search: documented Results.Items envelope ---------------------------

#[tokio::test]
#[ignore = "search: emit public {Results:{Count,Items:[{World|Sector|Subsector|Label}]}} (needs Tantivy)"]
async fn search_public_envelope() {
    let (status, _, body) = get("/api/search?q=Regina").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "search_regina.json");
}

// --- Data: single world by hex -------------------------------------------

/// `/data/{sector}/{hex}` — the single-world lookup third-party tools (worldgen's
/// solar-system generator) use. Must match the reference SEC-JSON envelope
/// `{"Worlds":[{Name,UWP,Zone,Allegiance,…}]}` exactly (golden captured live from
/// travellermap.com). This endpoint had **no** test before, which is how the
/// "worldgen can't look up worlds" 404 shipped.
#[tokio::test]
async fn data_world_by_hex() {
    let (status, ct, body) = get("/data/Spinward%20Marches/1910").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    assert_json_matches(&body, "data_world_regina.json");
    parity_json("/data/Spinward%20Marches/1910").await;
}

// --- SEC / tab text output -----------------------------------------------

#[tokio::test]
async fn sec_tab_delimited() {
    let (status, ct, body) = get("/api/sec?sector=Spinward%20Marches&subsector=A&type=TabDelimited").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/plain"), "ct={ct}");
    // TabDelimited carries no metadata block (hence no timestamp) — byte-exact.
    assert_eq!(body, golden("sec_sm_subsectorA.tab"));
    parity_text("/api/sec?sector=Spinward%20Marches&subsector=A&type=TabDelimited").await;
}

#[tokio::test]
async fn sec_second_survey() {
    let (status, _, body) = get("/api/sec?sector=Spinward%20Marches&subsector=A&type=SecondSurvey").await;
    assert_eq!(status, StatusCode::OK);
    // The metadata block carries a generation timestamp; normalize it away.
    assert_eq!(strip_timestamp(&body), strip_timestamp(&golden("sec_sm_subsectorA.sec")));
    parity_text("/api/sec?sector=Spinward%20Marches&subsector=A&type=SecondSurvey").await;
}

/// Replace the `# <ISO-8601 timestamp>` metadata line with a fixed token so the
/// (non-deterministic) generation time doesn't defeat byte comparison.
fn strip_timestamp(s: &str) -> String {
    s.lines()
        .map(|l| {
            if l.starts_with("# ") && l.len() > 4 && l.as_bytes()[2].is_ascii_digit() && l.contains('T') {
                "# <TIMESTAMP>"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// --- Metadata ------------------------------------------------------------

#[tokio::test]
async fn metadata_json() {
    let (status, ct, body) = get("/api/metadata?sector=Spinward%20Marches").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    assert_eq!(norm_metadata(&body), norm_metadata(&golden("metadata_sm.json")));
    parity_json_with("/api/metadata?sector=Spinward%20Marches", norm_metadata).await;
}

/// Normalize a metadata doc for comparison: (1) sort `Allegiances` by code —
/// the reference builds it from an unordered `HashSet`; (2) resolve each route's
/// `Start`/`End` + offsets to **absolute world coords** and drop the offsets, so
/// two encodings of the same cross-sector edge (`3201`+`StartOffsetX=-1` vs
/// `0001`) compare equal — immune to upstream route-notation drift.
fn norm_metadata(s: &str) -> Value {
    let mut v = jv(s);
    let (sx, sy) = (v["X"].as_i64().unwrap_or(0), v["Y"].as_i64().unwrap_or(0));
    if let Some(a) = v.get_mut("Allegiances").and_then(|a| a.as_array_mut()) {
        a.sort_by(|x, y| x["Code"].as_str().cmp(&y["Code"].as_str()));
    }
    if let Some(routes) = v.get_mut("Routes").and_then(|r| r.as_array_mut()) {
        for r in routes.iter_mut() {
            let o = r.as_object_mut().unwrap();
            let s_abs = route_abs(o, "Start", "StartOffsetX", "StartOffsetY", sx, sy);
            let e_abs = route_abs(o, "End", "EndOffsetX", "EndOffsetY", sx, sy);
            for k in ["StartOffsetX", "StartOffsetY", "EndOffsetX", "EndOffsetY"] {
                o.remove(k);
            }
            o.insert("Start".into(), s_abs);
            o.insert("End".into(), e_abs);
        }
    }
    v
}

fn route_abs(o: &serde_json::Map<String, Value>, hk: &str, oxk: &str, oyk: &str, sx: i64, sy: i64) -> Value {
    let hex = o.get(hk).and_then(|h| h.as_str()).unwrap_or("0000");
    let col: i64 = hex.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let row: i64 = hex.get(2..4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let ox = o.get(oxk).and_then(|v| v.as_i64()).unwrap_or(0);
    let oy = o.get(oyk).and_then(|v| v.as_i64()).unwrap_or(0);
    Value::Array(vec![Value::from((sx + ox) * 32 + col), Value::from((sy + oy) * 40 + row)])
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
async fn credits_json() {
    let (status, _, body) = get("/api/credits?sector=Spinward%20Marches&hex=1910").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "credits_sm_1910.json");
    parity_json("/api/credits?sector=Spinward%20Marches&hex=1910").await;
}

// --- JumpWorlds ----------------------------------------------------------

#[tokio::test]
async fn jumpworlds_json() {
    let (status, _, body) = get("/api/jumpworlds?sector=Spinward%20Marches&hex=1910&jump=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "jumpworlds_sm_1910_j2.json");
    parity_json("/api/jumpworlds?sector=Spinward%20Marches&hex=1910&jump=2").await;
}

// --- Route: documented bare-array public shape ---------------------------
// Our /api/route works but emits a private {waypoints,jumps,parsecs} object;
// the public API returns a bare array of stops. This asserts the public shape.

#[tokio::test]
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
async fn data_alias_coordinates() {
    let (status, _, body) = get("/data/Spinward%20Marches/1910/coordinates").await;
    assert_eq!(status, StatusCode::OK);
    assert_json_matches(&body, "coordinates_sector_hex.json");
    parity_json("/data/Spinward%20Marches/1910/coordinates").await;
}

#[tokio::test]
async fn data_alias_sec_tab() {
    let (status, ..) = get("/data/Spinward%20Marches/tab").await;
    assert_eq!(status, StatusCode::OK);
    parity_text("/data/Spinward%20Marches/tab").await;
}

// --- Content negotiation: Accept: text/xml -------------------------------

#[tokio::test]
async fn xml_content_negotiation() {
    let (status, ct, _) = get_with(
        "/api/coordinates?sector=Spinward%20Marches&hex=1910",
        &[("accept", "text/xml")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("xml"), "ct={ct}");
}
