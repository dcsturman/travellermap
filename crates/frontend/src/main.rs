//! Traveller Map client — Leptos, client-side rendering, compiled to WASM.
//!
//! Roams charted space: fetches the universe index + macro overlays, streams
//! the sectors overlapping the viewport, and renders client-side with LOD
//! styling. The canvas fills the window (device-pixel-ratio aware). All drawing
//! goes through `render` → `trait Canvas` (see `canvas.rs`).

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use tmap_core::astrometrics::{parse_hex, PARSEC_SCALE_X};
use tmap_core::dto::{
    Overlays, RouteResult, SearchItem, SearchResults, SectorData, UniverseResult, World,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

mod canvas;
mod glyph;
mod render;
mod route_print;
mod world_panel;
mod world_print;
use render::ViewState;
use world_panel::{subsector_letter, SelectedWorld, WorldPanel};

/// Default milieu (Imperial year 1105, "The Golden Age").
const DEFAULT_MILIEU: &str = "M1105";
/// The curated era snapshots offered by the milieu selector: `(code, display
/// label)`. A hardcoded OTU subset ported from the reference `index.html`
/// `#settings` milieu radios — the data (`milieu.tab`) only lists which XMLs
/// exist + an OTU tag, not these labels (and includes M600, which we omit).
const MILIEUX: &[(&str, &str)] = &[
    ("IW", "The Interstellar Wars"),
    ("M0", "Milieu 0 – Early Imperium"),
    ("M990", "990 – Solomani Rim War"),
    ("M1105", "1105 – The Golden Age"),
    ("M1120", "1120 – The Rebellion"),
    ("M1201", "1201 – The New Era"),
    ("M1248", "1248 – The New, New Era"),
    ("M1900", "1900 – The Far Far Future"),
];
/// Where the map opens — Spinward Marches grid coords.
const START: (i32, i32) = (-4, -1);
/// Safety cap: when zoomed out far enough to see more than this many sectors,
/// don't stream per-sector (Phase 5 macro overlays cover that range).
const MAX_STREAM: usize = 48;

/// A flattened search hit for the results list: display name, where it lives, and
/// the absolute world hex `(col, row)` to center on. Built from the public
/// [`SearchItem`] envelope (`{"World"|"Sector"|"Subsector":{…}}`).
#[derive(Clone, PartialEq)]
struct Hit {
    name: String,
    sector: String,
    hex: Option<String>,
    coord: (i32, i32),
}

impl Hit {
    fn from_item(item: SearchItem) -> Hit {
        match item {
            SearchItem::World(w) => Hit {
                name: w.name,
                sector: w.sector,
                hex: Some(format!("{:02}{:02}", w.hex_x, w.hex_y)),
                coord: (w.sector_x * 32 + w.hex_x, w.sector_y * 40 + w.hex_y),
            },
            SearchItem::Sector(s) => Hit {
                name: s.name.clone(),
                sector: s.name,
                hex: None,
                // Sector center (hex 16,20 of a 32×40 sector).
                coord: (s.sector_x * 32 + 16, s.sector_y * 40 + 20),
            },
            SearchItem::Subsector(s) => {
                // Subsector center: 4×4 grid of 8×10-parsec cells, index A–P.
                let i = (s.index.chars().next().unwrap_or('A') as u8).saturating_sub(b'A') as i32;
                Hit {
                    name: s.name,
                    sector: s.sector,
                    hex: None,
                    coord: (
                        s.sector_x * 32 + (i % 4) * 8 + 4,
                        s.sector_y * 40 + (i / 4) * 10 + 5,
                    ),
                }
            }
            // A labeled region (e.g. "Outrim Void") — center on its anchor hex.
            SearchItem::Label(l) => Hit {
                name: l.name,
                sector: String::new(),
                hex: None,
                coord: (l.sector_x * 32 + l.hex_x, l.sector_y * 40 + l.hex_y),
            },
        }
    }
}

/// Shared style for the top-right control buttons (home / key / hamburger).
const BTN_STYLE: &str = "width:40px; height:38px; border:none; border-radius:6px; \
    background:rgba(40,44,58,0.92); color:#e6ecf7; font-size:18px; line-height:1; \
    cursor:pointer; box-shadow:0 1px 4px rgba(0,0,0,0.5);";
/// Shared style for the floating panels (legend / settings).
const PANEL_STYLE: &str = "position:fixed; top:56px; right:12px; width:300px; \
    max-height:78dvh; overflow:auto; box-sizing:border-box; padding:14px 18px 18px; \
    background:rgba(12,14,22,0.96); border:1px solid #2a3145; border-radius:10px; \
    color:#cfd6e6; font:14px system-ui,sans-serif; box-shadow:0 6px 24px rgba(0,0,0,0.6);";

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

fn win() -> web_sys::Window {
    web_sys::window().expect("no window")
}

/// Open a self-printing HTML document in a new tab via a Blob URL. Blob URLs are
/// reliable across browsers where top-level `document.write`/`data:` are
/// flaky/blocked; the document self-prints on load (see the print builders).
fn open_print_html(html: &str) {
    if html.is_empty() {
        return;
    }
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(html));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type("text/html");
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag) else { return };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
    let _ = win().open_with_url_and_target(&url, "_blank");
}

/// Base URL of the external worldgen solar-system image service (Callisto). The
/// double-click popup calls this directly — travellermap has no worldgen
/// dependency. Change here (or later make it configurable) to point elsewhere.
#[cfg(feature = "callisto")]
const SYSTEM_SERVICE: &str = "https://tools.callistoflight.com/api/system";

/// Base URL of the external worldgen planet-surface image service (Callisto). The
/// "World Map" button in the detail panel calls this directly. Same deterministic
/// seed chain + GCS cache as `/api/system`, so the first render of a given world
/// can take 20–30 s but is instant thereafter.
#[cfg(feature = "callisto")]
const WORLD_SERVICE: &str = "https://tools.callistoflight.com/api/world";

/// State of the full-screen Callisto image popup (dev-only). One popup serves
/// both the double-click solar-system render and the detail-panel world-surface
/// render; the variant drives whether we show a spinner, the zoom/pan viewer, or
/// an error card. `Ready` carries `(object_url, service_url, title)` — the object
/// URL backs the `<img>`/download, the service URL backs Print.
#[cfg(feature = "callisto")]
#[derive(Clone)]
enum ImgView {
    /// Render in flight — show a spinner + elapsed-seconds counter.
    Loading { title: String },
    /// Render arrived — zoom/pan viewer over the PNG.
    Ready { obj: String, svc: String, title: String },
    /// Render failed (unreachable service, or a 422 from a partial/placeholder UWP).
    Error { title: String, msg: String },
}

/// Wrap PNG bytes in an object URL for the solar-system popup `<img>` /
/// download. Fetching the bytes (rather than pointing `<img>` at the remote
/// service) lets us open the popup only on a real image, and makes a download
/// work for the cross-origin service (the `download` attribute is ignored for
/// cross-origin URLs, but honored for an object URL). Revoke it when the popup
/// closes.
#[cfg(feature = "callisto")]
fn blob_url_from_png(bytes: &[u8]) -> Option<String> {
    // Defend against a non-image 200 (e.g. an SPA fallback) — require PNG magic.
    if bytes.len() < 8 || &bytes[1..4] != b"PNG" {
        return None;
    }
    let arr = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&arr);
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type("image/png");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

/// Trigger a browser download of an (object) URL as a file (`<a download>`).
#[cfg(feature = "callisto")]
fn download_url(url: &str, filename: &str) {
    let Some(doc) = win().document() else { return };
    let Ok(a) = doc.create_element("a") else { return };
    let _ = a.set_attribute("href", url);
    let _ = a.set_attribute("download", filename);
    if let Ok(a) = a.dyn_into::<web_sys::HtmlElement>() {
        a.click();
    }
}

/// Open a print window for an absolute image URL (embedded in a self-printing
/// doc). Uses the remote service URL, not the object URL — a blob: print doc has
/// an opaque origin and can't resolve another document's object URL, but a plain
/// cross-origin `<img>` loads fine.
#[cfg(feature = "callisto")]
fn print_image_url(url: &str, title: &str) {
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=utf-8><title>{title}</title>\
         <style>body{{margin:0;padding:18px;text-align:center;font:14px system-ui,sans-serif;color:#000;}}\
           h1{{font-size:18px;margin:0 0 12px;}}img{{max-width:100%;}}\
           @media print{{body{{padding:0;}}}}</style></head><body>\
         <h1>{title}</h1><img src=\"{url}\" onload=\"window.print()\">\
         </body></html>"
    );
    open_print_html(&html);
}

/// Start a 1 Hz timer that increments `elapsed` (reset to 0 first), returning the
/// interval handle so the caller can stop it with `clear_interval_with_handle`.
/// The tick closure is leaked (`forget`) — one tiny leak per popup open, which is
/// fine for this dev-only feature; clearing the handle stops the ticks regardless.
#[cfg(feature = "callisto")]
fn start_elapsed_timer(elapsed: RwSignal<u32>) -> i32 {
    elapsed.set(0);
    let cb = Closure::<dyn FnMut()>::new(move || elapsed.update(|n| *n += 1));
    let id = win()
        .set_interval_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 1000)
        .unwrap_or(-1);
    cb.forget();
    id
}

/// Open the popup on `url`: show the spinner immediately, fetch in the background,
/// then swap to the zoom/pan viewer (success) or an error card (failure). A
/// generation counter (`gen`) guards against a stale fetch landing after the user
/// closed the popup or launched a newer render. Shared by the solar-system
/// double-click and the world-surface button.
#[cfg(feature = "callisto")]
#[allow(clippy::too_many_arguments)]
fn launch_render(
    system_view: RwSignal<Option<ImgView>>,
    sys_zoom: RwSignal<f64>,
    sys_pan: RwSignal<(f64, f64)>,
    sys_elapsed: RwSignal<u32>,
    sys_timer: RwSignal<Option<i32>>,
    sys_gen: RwSignal<u64>,
    url: String,
    title: String,
) {
    // Revoke a prior object URL and stop a prior timer so nothing leaks.
    if let Some(ImgView::Ready { obj, .. }) = system_view.get_untracked() {
        let _ = web_sys::Url::revoke_object_url(&obj);
    }
    if let Some(id) = sys_timer.get_untracked() {
        win().clear_interval_with_handle(id);
    }
    sys_zoom.set(1.0);
    sys_pan.set((0.0, 0.0));
    let gen = sys_gen.get_untracked().wrapping_add(1);
    sys_gen.set(gen);
    system_view.set(Some(ImgView::Loading { title: title.clone() }));
    sys_timer.set(Some(start_elapsed_timer(sys_elapsed)));

    spawn_local(async move {
        let result = gloo_net::http::Request::get(&url).send().await;
        // A newer launch or a close bumped the generation — drop this result.
        if sys_gen.get_untracked() != gen {
            return;
        }
        if let Some(id) = sys_timer.get_untracked() {
            win().clear_interval_with_handle(id);
            sys_timer.set(None);
        }
        let state = match result {
            Ok(resp) if resp.ok() => match resp.binary().await {
                Ok(bytes) => match blob_url_from_png(&bytes) {
                    Some(obj) => ImgView::Ready { obj, svc: url, title },
                    None => ImgView::Error {
                        title,
                        msg: "The service returned data that wasn't a PNG image.".into(),
                    },
                },
                Err(_) => ImgView::Error { title, msg: "Couldn't read the image data.".into() },
            },
            // Non-2xx: surface the service's plain-text reason (422 = bad/partial UWP).
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let msg = if body.trim().is_empty() {
                    format!("The map service returned HTTP {status}.")
                } else {
                    body
                };
                ImgView::Error { title, msg }
            }
            Err(_) => ImgView::Error { title, msg: "Couldn't reach the map service.".into() },
        };
        // Re-check generation after the await on the body, then apply.
        if sys_gen.get_untracked() == gen {
            system_view.set(Some(state));
        }
    });
}

/// Trigger a browser download of a canvas as a PNG (via a data-URL `<a download>`).
fn download_canvas_png(canvas: &HtmlCanvasElement, filename: &str) {
    let Ok(url) = canvas.to_data_url_with_type("image/png") else { return };
    let Some(doc) = win().document() else { return };
    let Ok(a) = doc.create_element("a") else { return };
    let _ = a.set_attribute("href", &url);
    let _ = a.set_attribute("download", filename);
    if let Ok(a) = a.dyn_into::<web_sys::HtmlElement>() {
        a.click();
    }
}

/// Open a print window for a canvas (PNG embedded in a minimal self-printing doc).
fn print_canvas(canvas: &HtmlCanvasElement, title: &str) {
    let Ok(url) = canvas.to_data_url_with_type("image/png") else { return };
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=utf-8><title>{title}</title>\
         <style>body{{margin:0;padding:24px;text-align:center;font:14px system-ui,sans-serif;color:#000;}}\
           h1{{font-size:18px;margin:0 0 14px;}}img{{max-width:100%;border:1px solid #000;}}\
           @media print{{body{{padding:0;}}}}</style></head><body>\
         <h1>{title}</h1><img src=\"{url}\">\
         <script>window.onload=function(){{window.print();}}</script></body></html>"
    );
    open_print_html(&html);
}

/// Strip HTML tags from a credits string (roxmltree already decoded entities),
/// collapsing whitespace to a single readable line for the footer.
fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A small underlined section header in a panel.
fn section_header(label: &'static str) -> impl IntoView {
    view! {
        <div style="font-weight:700; color:#aab3c8; letter-spacing:0.04em; \
                    margin:14px 0 5px; padding-bottom:3px; border-bottom:1px solid #2a3145;">
            {label}
        </div>
    }
}

/// One labeled on/off switch row in the settings menu.
fn toggle_row(label: &'static str, sig: RwSignal<bool>) -> impl IntoView {
    view! {
        <label style="display:flex; align-items:center; gap:10px; padding:8px 0; \
                      cursor:pointer; border-bottom:1px solid #20283a;">
            <input type="checkbox" prop:checked=move || sig.get()
                   on:change=move |ev| sig.set(event_target_checked(&ev))
                   style="width:16px; height:16px; accent-color:#e32736; cursor:pointer;" />
            <span style="color:#e9eef9; font-weight:600;">{label}</span>
        </label>
    }
}

/// One color-swatch row in the legend's world-characteristics list.
fn swatch(color: &'static str, outline: bool, label: &'static str) -> impl IntoView {
    let border = if outline { "1px solid #fff" } else { "none" };
    view! {
        <div style="display:flex; align-items:center; gap:10px; padding:3px 0;">
            <span style=format!("display:inline-block; width:14px; height:14px; flex:none; \
                   border-radius:50%; background:{color}; border:{border};")></span>
            <span style="color:#dfe5f2;">{label}</span>
        </div>
    }
}

/// One symbol-meaning row in the legend.
fn legend_row(sym: &'static str, sym_color: &'static str, label: &'static str) -> impl IntoView {
    view! {
        <div style="display:flex; align-items:baseline; gap:10px; padding:3px 0;">
            <span style=format!("display:inline-block; width:38px; flex:none; text-align:center; \
                   font-weight:700; color:{sym_color};")>{sym}</span>
            <span style="color:#dfe5f2;">{label}</span>
        </div>
    }
}

/// Size the canvas drawing buffer to its *rendered CSS box* × devicePixelRatio
/// (crisp on retina). Returns the buffer size in device pixels.
///
/// We measure the canvas's own `clientWidth/Height`, not `window.inner*`: on iOS
/// Safari the visual viewport (`inner_height`) lags the layout box (`100dvh`)
/// while the toolbar animates, and sizing the buffer to the window while the CSS
/// box is a different height makes the browser stretch the buffer to fit —
/// the map looks oversized and its edges get clipped. The client box is what the
/// canvas is actually displayed at, so a 1:1 buffer never stretches. (Fall back
/// to the window before first layout, when the client box reads 0.)
fn size_canvas(canvas: &HtmlCanvasElement) -> (u32, u32) {
    let w = win();
    let dpr = w.device_pixel_ratio();
    let cw = match canvas.client_width() {
        0 => w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0),
        n => n as f64,
    };
    let ch = match canvas.client_height() {
        0 => w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(768.0),
        n => n as f64,
    };
    let bw = ((cw * dpr).round() as u32).max(1);
    let bh = ((ch * dpr).round() as u32).max(1);
    canvas.set_width(bw);
    canvas.set_height(bh);
    (bw, bh)
}

/// An in-progress touch gesture on the map canvas. iOS Safari synthesizes mouse
/// events only for *taps* (so single-tap world-select rides the mouse path), but
/// never for drags or pinches — so pan and zoom are driven from raw touches here.
/// `One` remembers the last finger position for panning; `Two` remembers the
/// prior pinch distance + midpoint so each move folds pan and zoom into one
/// transform.
#[derive(Clone, Copy)]
enum TouchGesture {
    One { last: (f64, f64) },
    Two { dist: f64, mid: (f64, f64) },
}

/// All active touch points as CSS-pixel coordinates.
fn touch_points(ev: &web_sys::TouchEvent) -> Vec<(f64, f64)> {
    let list = ev.touches();
    (0..list.length())
        .filter_map(|i| list.get(i))
        .map(|t| (t.client_x() as f64, t.client_y() as f64))
        .collect()
}

fn pt_dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn pt_mid(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

/// Schedule a one-shot timer; returns its handle so it can be cancelled. Used to
/// debounce the share-URL update and (callisto) to detect a long-press.
fn set_timeout(ms: i32, f: impl FnMut() + 'static) -> i32 {
    let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(f);
    let id = win()
        .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), ms)
        .unwrap_or(0);
    cb.forget(); // one-shot — leak the tiny closure rather than track its lifetime
    id
}

fn clear_timeout(id: i32) {
    win().clear_timeout_with_handle(id);
}

/// How long a stationary finger must be held to trigger the solar-system view,
/// and how far it may drift before that's treated as a pan instead.
#[cfg(feature = "callisto")]
const LONG_PRESS_MS: i32 = 500;
#[cfg(feature = "callisto")]
const LONG_PRESS_SLOP: f64 = 10.0;

/// The canvas's CSS (logical) size — the coordinate space we draw in (the
/// context is DPR-scaled in `render::draw`).
fn logical_dims(canvas: &HtmlCanvasElement) -> (f64, f64) {
    (
        canvas.client_width().max(1) as f64,
        canvas.client_height().max(1) as f64,
    )
}

/// Build the shareable URL for the current view. **Our own param scheme**
/// (`cx`,`cy` = center in parsec space, `scale` = px/parsec, `milieu` unless the
/// default) — deliberately encapsulated here so swapping in a travellermap.com-
/// compatible `p=x!y!logScale` format later is a one-function change. Returns the
/// absolute URL (origin + path + query) so it's directly shareable/embeddable.
/// Fixed precision keeps the URL short and free of float noise.
fn build_share_url(view: ViewState, milieu: &str) -> String {
    let loc = win().location();
    let origin = loc.origin().unwrap_or_default();
    let path = loc.pathname().unwrap_or_else(|_| "/".to_string());
    let mut q = format!(
        "?cx={:.3}&cy={:.3}&scale={:.2}",
        view.center.0, view.center.1, view.scale,
    );
    if milieu != DEFAULT_MILIEU {
        q.push_str("&milieu=");
        q.push_str(milieu);
    }
    format!("{origin}{path}{q}")
}

/// Parse the initial view + milieu from the page URL's query (inverse of
/// [`build_share_url`]). Either may be absent; an unknown milieu is ignored.
fn parse_share_params() -> (Option<ViewState>, Option<&'static str>) {
    let Ok(search) = win().location().search() else {
        return (None, None);
    };
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else {
        return (None, None);
    };
    let num = |k: &str| params.get(k).and_then(|s| s.parse::<f64>().ok());
    let view = match (num("cx"), num("cy"), num("scale")) {
        (Some(cx), Some(cy), Some(scale)) if scale > 0.0 => Some(ViewState {
            center: (cx, cy),
            scale: scale.clamp(render::MIN_SCALE, render::MAX_SCALE),
        }),
        _ => None,
    };
    // Map the milieu string back to one of our known &'static codes.
    let milieu = params
        .get("milieu")
        .and_then(|m| MILIEUX.iter().map(|(c, _)| *c).find(|c| *c == m));
    (view, milieu)
}

/// Fetch + decode a JSON value from the backend (proxied via Trunk at /api).
async fn fetch_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    let resp = gloo_net::http::Request::get(url)
        // Always revalidate with the server (cheap 304 if unchanged) so a
        // stale browser-cached response can never mask a backend change.
        .cache(web_sys::RequestCache::NoCache)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

/// Fetch a computed jump route from the backend `/api/route` endpoint. `start`
/// and `end` are `"Sector Name 0101"` strings; `jump` is the drive rating.
async fn fetch_route(
    start: &str,
    end: &str,
    jump: i32,
    milieu: &str,
    opts: (bool, bool, bool, bool),
) -> Result<RouteResult, String> {
    let s = String::from(js_sys::encode_uri_component(start));
    let e = String::from(js_sys::encode_uri_component(end));
    // `detail=true` asks for the rich {waypoints,jumps,parsecs} shape (absolute
    // coords for drawing); the public default is a bare array of stops.
    let mut url = format!("/api/route?start={s}&end={e}&jump={jump}&milieu={milieu}&detail=true");
    // (wild, im, nored, aok) — append only when set; the backend defaults false.
    let (wild, im, nored, aok) = opts;
    if wild {
        url.push_str("&wild=true");
    }
    if im {
        url.push_str("&im=true");
    }
    if nored {
        url.push_str("&nored=true");
    }
    if aok {
        url.push_str("&aok=true");
    }
    fetch_json::<RouteResult>(&url).await
}

#[component]
fn App() -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let (status, set_status) = signal("Loading universe…".to_string());
    // A share/permalink in the URL (?cx&cy&scale&milieu) seeds the initial view
    // and milieu; both fall back to defaults when absent. See `parse_share_params`.
    let (url_view, url_milieu) = parse_share_params();
    // Active milieu (era snapshot). Changing it tears down and re-streams the
    // universe for that era (see the universe-load effect); per-milieu caches and
    // overlays differ, but the macro overlays are milieu-independent so they stay.
    let milieu = RwSignal::new(url_milieu.unwrap_or(DEFAULT_MILIEU));
    let (view, set_view) = signal(None::<ViewState>);
    let (results, set_results) = signal(Vec::<Hit>::new());
    let drag = RwSignal::new(None::<(f64, f64)>);
    // True while the cursor hovers directly over a world (callisto only): flips
    // the map cursor to an arrow to signal it's clickable / double-clickable.
    // Stays false in non-callisto builds (never written), so the cursor is
    // unchanged there.
    let hover_world = RwSignal::new(false);
    // Canvas buffer size (device px); changes on mount and window resize.
    let (canvas_size, set_canvas_size) = signal((0u32, 0u32));

    // Off-reactive caches (read by reference in draw; `version` triggers redraws).
    let index = StoredValue::new(HashMap::<(i32, i32), String>::new()); // coord → name
    let sectors = StoredValue::new(HashMap::<(i32, i32), SectorData>::new());
    let inflight = StoredValue::new(HashSet::<(i32, i32)>::new());
    let failed = StoredValue::new(HashSet::<(i32, i32)>::new()); // sectors that errored — don't retry
    let overlays = StoredValue::new(None::<Overlays>);
    let route = RwSignal::new(None::<RouteResult>); // computed jump route (reactive: draws + lists)
    let (version, set_version) = signal(0u32);
    let (index_ready, set_index_ready) = signal(false);

    // Data-source credit for the sector under the viewport center — the footer's
    // dynamic left text (recomputes as you pan/zoom or sectors stream in).
    let footer_credit = Memo::new(move |_| {
        let _ = version.get();
        let Some(v) = view.get() else { return String::new() };
        let wc = (v.center.0 / PARSEC_SCALE_X as f64).round() as i32;
        let wr = v.center.1.round() as i32;
        let cell = ((wc - 1).div_euclid(32), (wr - 1).div_euclid(40));
        sectors.with_value(|m| {
            m.get(&cell)
                .and_then(|s| s.info.credits.as_deref())
                .map(strip_html)
                .unwrap_or_default()
        })
    });

    // Shareable URL for the current view (shown live in the Share panel) and the
    // address-bar permalink. The panel field updates every frame, but the
    // history write is DEBOUNCED: Safari rate-limits replaceState (~100/30s) and a
    // single drag fires far more, so we only rewrite the URL once movement settles.
    let share_url = RwSignal::new(String::new());
    let url_timer = RwSignal::new(None::<i32>);
    Effect::new(move |_| {
        let Some(v) = view.get() else { return };
        let url = build_share_url(v, milieu.get());
        share_url.set(url.clone()); // live for the panel
        if let Some(id) = url_timer.get_untracked() {
            clear_timeout(id);
        }
        let id = set_timeout(400, move || {
            url_timer.set(None);
            if let Ok(hist) = win().history() {
                let _ = hist.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
            }
        });
        url_timer.set(Some(id));
    });

    // Jump-route planner state.
    let route_open = RwSignal::new(false); // planner panel visible (squiggle toggle)
    let route_hover = RwSignal::new(false); // hover state for the toggle (icon → red)
    let route_start = RwSignal::new(String::new());
    let route_end = RwSignal::new(String::new());
    let route_jump = RwSignal::new(0i32); // 0 = none chosen yet (a J-N pill picks it)
    // Route-finding option toggles (the reference's `routeOptions` checkboxes);
    // `/api/route` accepts each as a flag. Off by default.
    let route_wild = RwSignal::new(false); // stops must have gas giant or water (wilderness refuel)
    let route_im = RwSignal::new(false); // stops must be Imperial member worlds
    let route_nored = RwSignal::new(false); // avoid TAS Red Zones
    let route_aok = RwSignal::new(false); // allow anomalies / calibration points as stops

    // World detail panel: the clicked world (overview-LOD until the on-demand
    // `?lod=full` fetch upgrades it), plus a per-sector full-LOD cache so a
    // second click on the same sector skips the round-trip.
    let selected = RwSignal::new(None::<SelectedWorld>);
    let full_sectors = StoredValue::new(HashMap::<(i32, i32), SectorData>::new());
    // Active jump-N neighborhood cutout: (selected-world identity, world name,
    // origin Coord, jump). The identity `(sector_coord, hex)` ties it to a
    // specific world so dismissing / re-selecting clears it; `jump` (1..=6) is
    // the active rating. Rendered into its own overlay canvas (jumpmap_ref).
    let jumpmap =
        RwSignal::new(None::<((i32, i32), String, tmap_core::astrometrics::Coord, i32)>);
    let jumpmap_ref = NodeRef::<leptos::html::Canvas>::new();
    // Callisto (dev-only): the solar-system image popup for a double-clicked
    // world — `(object_url, service_url, title)`. The image is generated by the
    // external worldgen service (no travellermap dependency on worldgen); we fetch
    // it as a blob so the popup opens only on a real image and a cross-origin
    // download works (object_url), while Print uses the absolute service_url. The
    // popup is a zoom/pan viewer (the render is high-res); `sys_zoom`/`sys_pan`
    // hold the view transform and `sys_drag` the in-progress pan.
    #[cfg(feature = "callisto")]
    let system_view = RwSignal::new(None::<ImgView>);
    #[cfg(feature = "callisto")]
    let sys_zoom = RwSignal::new(1.0_f64);
    #[cfg(feature = "callisto")]
    let sys_pan = RwSignal::new((0.0_f64, 0.0_f64));
    #[cfg(feature = "callisto")]
    let sys_drag = RwSignal::new(None::<(f64, f64, f64, f64)>);
    // Loading-spinner support for the popup: elapsed-seconds counter, the active
    // tick-timer handle, and a generation counter that invalidates a stale fetch
    // (closed/superseded popup) so its late result is ignored.
    #[cfg(feature = "callisto")]
    let sys_elapsed = RwSignal::new(0_u32);
    #[cfg(feature = "callisto")]
    let sys_timer = RwSignal::new(None::<i32>);
    #[cfg(feature = "callisto")]
    let sys_gen = RwSignal::new(0_u64);
    let (route_status, set_route_status) = signal(String::new());
    // Distinguish a click (set endpoint) from a drag (pan): remember press origin.
    let down_pos = RwSignal::new(None::<(f64, f64)>);

    // Layer toggles (hamburger menu) — each redraws when flipped.
    let opt_galactic = RwSignal::new(true);
    let opt_grid = RwSignal::new(true);
    let opt_sector_names = RwSignal::new(true);
    let opt_borders = RwSignal::new(true);
    let opt_routes = RwSignal::new(true);
    let opt_region_names = RwSignal::new(true);
    let opt_important = RwSignal::new(true);
    let opt_filled = RwSignal::new(true);
    let opt_world_colors = RwSignal::new(true);
    let opt_dim = RwSignal::new(false);
    let opt_perf = RwSignal::new(false);
    // Which floating panel is open: 0 none, 1 legend (key), 2 settings (menu).
    let panel = RwSignal::new(0u8);

    // Keep the jump-N cutout tied to the selected world: if the selection is
    // cleared (panel close / empty-space click) or moves to a different world,
    // close a cutout that was pinned to the old one.
    Effect::new(move |_| {
        let cur = selected.get().map(|s| (s.sector_coord, s.world.hex.clone()));
        jumpmap.update(|j| {
            if let Some((coord, hex, _, _)) = j {
                if cur.as_ref() != Some(&(*coord, hex.clone())) {
                    *j = None;
                }
            }
        });
    });

    // Render the jump-N neighborhood cutout into its own overlay canvas whenever
    // the active cutout (or the streamed world data) changes. Reuses the main
    // `render::draw` with a hex-bubble clip + flat background (cutout options:
    // no compass/dim/perf/sector-watermark, no computed route).
    Effect::new(move |_| {
        let _ = version.get(); // re-render as neighbor sectors stream in
        let Some((_, _, origin, jump)) = jumpmap.get() else { return };
        let Some(canvas_el) = jumpmap_ref.get() else { return };
        let (cw, ch) = (canvas_el.client_width().max(1) as f64, canvas_el.client_height().max(1) as f64);
        let dpr = win().device_pixel_ratio();
        canvas_el.set_width((cw * dpr).round().max(1.0) as u32);
        canvas_el.set_height((ch * dpr).round().max(1.0) as u32);
        let v = render::fit_jump_view(cw, ch, origin, jump);
        let opts = render::RenderOptions {
            galactic_direction: false,
            sector_names: false,
            region_names: false, // no subsector watermark / border labels in the cutout
            dim_unofficial: false,
            perf_hud: false,
            jump_clip: Some(render::JumpClip { center: origin, jump }),
            ..render::RenderOptions::default()
        };
        // The cutout spans a few sectors at most; draw from all loaded sectors
        // overlapping the bubble (cheap) — neighbors fill in as they stream.
        sectors.with_value(|loaded| {
            let refs: Vec<&SectorData> = loaded.values().collect();
            overlays.with_value(|ov| {
                index.with_value(|idx| {
                    render::draw(&canvas_el, &refs, ov.as_ref(), idx, v, opts, None);
                });
            });
        });
    });

    // 0) Size the canvas on mount, and re-size on window resize.
    Effect::new(move |_| {
        if let Some(cv) = canvas_ref.get() {
            set_canvas_size.set(size_canvas(&cv));
        }
    });
    let resize_cb = Closure::<dyn FnMut()>::new(move || {
        if let Some(cv) = canvas_ref.get_untracked() {
            set_canvas_size.set(size_canvas(&cv));
        }
    });
    let cb_ref = resize_cb.as_ref().unchecked_ref();
    win().add_event_listener_with_callback("resize", cb_ref).ok();
    // iOS Safari shows/hides its toolbar without reliably firing window "resize";
    // the visual-viewport "resize" does fire, so listen there too to keep the
    // backing buffer matched to the (dynamic) visible area — no stretch/clip.
    if let Some(vv) = win().visual_viewport() {
        vv.add_event_listener_with_callback("resize", cb_ref).ok();
    }
    resize_cb.forget(); // lives for the app's lifetime

    // Esc dismisses the callisto solar-system popup (back to the map view).
    #[cfg(feature = "callisto")]
    {
        let keydown_cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                if let Some(view) = system_view.get_untracked() {
                    if let ImgView::Ready { obj, .. } = &view {
                        let _ = web_sys::Url::revoke_object_url(obj);
                    }
                    if let Some(id) = sys_timer.get_untracked() {
                        win().clear_interval_with_handle(id);
                        sys_timer.set(None);
                    }
                    sys_gen.update(|g| *g = g.wrapping_add(1)); // invalidate any in-flight fetch
                    system_view.set(None);
                }
            }
        });
        win()
            .add_event_listener_with_callback("keydown", keydown_cb.as_ref().unchecked_ref())
            .ok();
        keydown_cb.forget();
    }

    // 1) Load the universe index for the active milieu — and reload it whenever
    //    the milieu changes. Reading `milieu.get()` subscribes this effect; on a
    //    switch it tears down all per-milieu state (index, streamed sectors,
    //    in-flight/failed sets, full-LOD cache, per-sector render geometry) and
    //    resets milieu-scoped UI (selection, jump-map, route, search), then
    //    re-fetches. The macro overlays are milieu-independent, so they stay.
    Effect::new(move |_| {
        let m = milieu.get();
        set_index_ready.set(false);
        index.set_value(HashMap::new());
        sectors.update_value(|s| s.clear());
        inflight.update_value(|i| i.clear());
        failed.update_value(|f| f.clear());
        full_sectors.update_value(|fs| fs.clear());
        render::clear_caches();
        selected.set(None);
        jumpmap.set(None);
        route.set(None);
        set_results.set(Vec::new());
        set_status.set(format!("Loading {m}…"));
        spawn_local(async move {
            match fetch_json::<UniverseResult>(&format!("/api/universe?milieu={m}")).await {
                Ok(u) => {
                    // Ignore a response that arrived after another switch.
                    if milieu.get_untracked() != m {
                        return;
                    }
                    // Public shape: flat X/Y + a Names array (canonical name first).
                    let map: HashMap<(i32, i32), String> = u
                        .sectors
                        .into_iter()
                        .map(|s| {
                            let name = s.names.into_iter().next().map(|n| n.text).unwrap_or_default();
                            ((s.x, s.y), name)
                        })
                        .collect();
                    set_status.set(format!("{m} — {} sectors · drag to pan, scroll to zoom", map.len()));
                    index.set_value(map);
                    set_index_ready.set(true);
                    set_version.update(|v| *v += 1); // redraw so sector names show
                }
                Err(e) => set_status.set(format!("Universe load failed: {e}")),
            }
        });
    });

    // 1b) Load the macro overlays once (charted-space borders/routes/rifts).
    spawn_local(async move {
        if let Ok(ov) = fetch_json::<Overlays>("/api/overlays").await {
            overlays.set_value(Some(ov));
            set_version.update(|v| *v += 1);
        }
    });

    // 2) Stream the sectors overlapping the viewport (cached, prefetch ring).
    Effect::new(move |_| {
        if !index_ready.get() {
            return;
        }
        let m = milieu.get_untracked(); // milieu change reloads via index_ready/version
        let Some(v) = view.get() else { return };
        let _ = canvas_size.get();
        // Re-run as batches arrive so a viewport larger than MAX_STREAM keeps
        // filling in (this effect otherwise only fires on pan/zoom).
        let _ = version.get();
        if v.scale < render::WORLD_MIN_SCALE {
            return; // zoomed out — macro overlays cover this, no per-sector fetch
        }
        let Some(canvas_el) = canvas_ref.get() else {
            return;
        };
        let (w, h) = logical_dims(&canvas_el);
        let needed = render::visible_sectors(&v, w, h);
        // Viewport-center cell (bbox midpoint of the visible range) — used to
        // fetch nearest-first.
        let (cx, cy) = needed.iter().fold((0i64, 0i64), |(ax, ay), (x, y)| {
            (ax + *x as i64, ay + *y as i64)
        });
        let n = needed.len().max(1) as i64;
        let (cx, cy) = ((cx / n) as i32, (cy / n) as i32);
        let mut to_fetch: Vec<((i32, i32), String)> = index.with_value(|idx| {
            needed
                .into_iter()
                .filter_map(|cell| idx.get(&cell).map(|name| (cell, name.clone())))
                .filter(|(cell, _)| {
                    !sectors.with_value(|s| s.contains_key(cell))
                        && !inflight.with_value(|i| i.contains(cell))
                        && !failed.with_value(|f| f.contains(cell))
                })
                .collect()
        });
        // Never bail on a big viewport: fetch the nearest-to-center unloaded
        // sectors up to the cap (bounds concurrent fetches), and the `version`
        // re-run above pulls in the rest as these land — so it converges instead
        // of leaving panned-to sectors permanently blank.
        to_fetch.sort_by_key(|((x, y), _)| {
            let (dx, dy) = ((x - cx) as i64, (y - cy) as i64);
            dx * dx + dy * dy
        });
        to_fetch.truncate(MAX_STREAM);
        for (cell, name) in to_fetch {
            inflight.update_value(|i| {
                i.insert(cell);
            });
            let encoded = String::from(js_sys::encode_uri_component(&name));
            spawn_local(async move {
                // `overview` LOD: drops fields not rendered until extreme zoom
                // (stellar/Ix/Ex/Cx/…) — smaller payloads, cached + CDN-friendly.
                let url = format!("/api/sector/{m}/{encoded}?lod=overview");
                let result = fetch_json::<SectorData>(&url).await;
                // Drop a response that arrived after a milieu switch (else stale
                // old-era data would persist, since loaded cells aren't re-fetched).
                if milieu.get_untracked() != m {
                    return;
                }
                match result {
                    Ok(data) => sectors.update_value(|s| {
                        s.insert(cell, data);
                    }),
                    // Remember failures (missing data / parse error) so we don't
                    // re-request them on every pan.
                    Err(_) => failed.update_value(|f| {
                        f.insert(cell);
                    }),
                }
                inflight.update_value(|i| {
                    i.remove(&cell);
                });
                set_version.update(|v| *v += 1);
            });
        }
    });

    // 3) Redraw on view / data / size / toggle change. Lazily frames the start
    //    sector. Reading each toggle here subscribes the effect, so flipping one
    //    re-renders.
    Effect::new(move |_| {
        let _ = version.get();
        let opts = render::RenderOptions {
            galactic_direction: opt_galactic.get(),
            sector_grid: opt_grid.get(),
            sector_names: opt_sector_names.get(),
            borders: opt_borders.get(),
            routes: opt_routes.get(),
            region_names: opt_region_names.get(),
            important_worlds: opt_important.get(),
            filled_borders: opt_filled.get(),
            more_world_colors: opt_world_colors.get(),
            dim_unofficial: opt_dim.get(),
            perf_hud: opt_perf.get(),
            jump_clip: None,
        };
        if canvas_size.get().0 == 0 {
            return; // not sized yet (subscribes to resize)
        }
        let Some(canvas_el) = canvas_ref.get() else {
            return;
        };
        let v = match view.get() {
            Some(v) => v,
            None => {
                let (lw, lh) = logical_dims(&canvas_el);
                // A shared link's view wins over the default Spinward-Marches fit.
                set_view.set(Some(
                    url_view.unwrap_or_else(|| render::fit_sector(lw, lh, START.0, START.1)),
                ));
                return;
            }
        };
        // Only render sectors overlapping the viewport (+ prefetch ring), not
        // every sector accumulated while panning — bounds per-frame work.
        let (lw, lh) = logical_dims(&canvas_el);
        let visible: std::collections::HashSet<(i32, i32)> =
            render::visible_sectors(&v, lw, lh).into_iter().collect();
        sectors.with_value(|loaded| {
            let refs: Vec<&SectorData> = loaded
                .iter()
                .filter(|(cell, _)| visible.contains(cell))
                .map(|(_, s)| s)
                .collect();
            overlays.with_value(|ov| {
                index.with_value(|idx| {
                    route.with(|r| {
                        render::draw(&canvas_el, &refs, ov.as_ref(), idx, v, opts, r.as_ref());
                    });
                });
            });
        });
    });

    // --- input (mutates the view signal). Mouse coords are CSS px; scale by
    //     devicePixelRatio to match the device-pixel drawing buffer. ---

    let on_down = move |ev: web_sys::MouseEvent| {
        let p = (ev.client_x() as f64, ev.client_y() as f64);
        drag.set(Some(p));
        down_pos.set(Some(p));
    };
    // Is the cursor directly over a world disc? (~0.4 parsec, tighter than the
    // click tolerance so the arrow means "on a world", not "near one"). Only
    // meaningful once worlds are individually drawn (detail zoom).
    #[cfg(feature = "callisto")]
    let world_under_cursor = move |px: (f64, f64)| -> bool {
        let Some(cv) = canvas_ref.get_untracked() else { return false };
        let (w, h) = logical_dims(&cv);
        let Some(v) = view.get_untracked() else { return false };
        if v.scale < render::WORLD_MIN_SCALE {
            return false;
        }
        let target = v.to_parsec(w, h, px);
        const R2: f64 = 0.4 * 0.4;
        let mut over = false;
        // Only scan sectors overlapping the viewport (not everything loaded), so
        // the per-mousemove cost is bounded by what's on screen.
        sectors.with_value(|loaded| {
            'outer: for sc in render::visible_sectors(&v, w, h) {
                let Some(s) = loaded.get(&sc) else { continue };
                let Some(loc) = s.info.location else { continue };
                for wld in &s.worlds {
                    let Some((col, row)) = parse_hex(&wld.hex) else { continue };
                    let (wx, wy) = render::sector_hex_parsec(loc.x, loc.y, col, row);
                    if (wx - target.0).powi(2) + (wy - target.1).powi(2) <= R2 {
                        over = true;
                        break 'outer;
                    }
                }
            }
        });
        over
    };
    let on_move = move |ev: web_sys::MouseEvent| {
        let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
        if let Some((lx, ly)) = drag.get_untracked() {
            drag.set(Some((x, y)));
            if let Some(v) = view.get_untracked() {
                set_view.set(Some(ViewState {
                    center: (v.center.0 - (x - lx) / v.scale, v.center.1 - (y - ly) / v.scale),
                    ..v
                }));
            }
            return;
        }
        // Not dragging: track whether we're over a world so the cursor can hint
        // clickability (callisto only — the double-click solar-system feature).
        #[cfg(feature = "callisto")]
        {
            let over = world_under_cursor((x, y));
            if hover_world.get_untracked() != over {
                hover_world.set(over);
            }
        }
    };
    // Click-to-set: hit-test a canvas click to the nearest loaded world (within
    // ~1 hex) and fill the next empty endpoint field (start, then destination).
    let fill_endpoint = move |px: (f64, f64)| {
        let Some(cv) = canvas_ref.get_untracked() else { return };
        let (w, h) = logical_dims(&cv);
        let Some(v) = view.get_untracked() else { return };
        let target = v.to_parsec(w, h, px);
        let mut best: Option<(f64, String)> = None;
        sectors.with_value(|loaded| {
            for s in loaded.values() {
                let Some(loc) = s.info.location else { continue };
                for wld in &s.worlds {
                    let Some((col, row)) = parse_hex(&wld.hex) else { continue };
                    let (wx, wy) = render::sector_hex_parsec(loc.x, loc.y, col, row);
                    let d = (wx - target.0).powi(2) + (wy - target.1).powi(2);
                    if d < best.as_ref().map_or(f64::MAX, |(bd, _)| *bd) {
                        let label = if wld.name.is_empty() {
                            format!("{} {}", s.info.name, wld.hex)
                        } else {
                            wld.name.clone()
                        };
                        best = Some((d, label));
                    }
                }
            }
        });
        let Some((d, label)) = best else { return };
        if d > 0.9 * 0.9 {
            return; // clicked empty space, not a world
        }
        if route_start.get_untracked().trim().is_empty() {
            route_start.set(label);
        } else if route_end.get_untracked().trim().is_empty() {
            route_end.set(label);
        } else {
            route_start.set(label); // both set — restart the selection
            route_end.set(String::new());
        }
    };
    // Click-to-select: hit-test a canvas click to the nearest loaded world and
    // open the detail panel for it. Mirrors `fill_endpoint`'s hit-test but keeps
    // the whole `World` (+ sector context), then fetches the sector at `?lod=full`
    // on demand to fill in the stellar/Ix/Ex/Cx/… fields the overview LOD omits.
    let select_world = move |px: (f64, f64)| {
        let Some(cv) = canvas_ref.get_untracked() else { return };
        let (w, h) = logical_dims(&cv);
        let Some(v) = view.get_untracked() else { return };
        let target = v.to_parsec(w, h, px);
        // (dist², world, sector name, sector coord, subsector name)
        let mut best: Option<(f64, World, String, (i32, i32), String)> = None;
        sectors.with_value(|loaded| {
            for s in loaded.values() {
                let Some(loc) = s.info.location else { continue };
                for wld in &s.worlds {
                    let Some((col, row)) = parse_hex(&wld.hex) else { continue };
                    let (wx, wy) = render::sector_hex_parsec(loc.x, loc.y, col, row);
                    let d = (wx - target.0).powi(2) + (wy - target.1).powi(2);
                    if d < best.as_ref().map_or(f64::MAX, |b| b.0) {
                        let letter = subsector_letter(col, row);
                        let sub = s
                            .info
                            .subsectors
                            .iter()
                            .find(|ss| ss.index.chars().next() == Some(letter))
                            .map(|ss| ss.name.clone())
                            .unwrap_or_else(|| format!("Subsector {letter}"));
                        best = Some((d, wld.clone(), s.info.name.clone(), (loc.x, loc.y), sub));
                    }
                }
            }
        });
        let Some((d, world, sector_name, sector_coord, subsector)) = best else { return };
        if d > 0.9 * 0.9 {
            selected.set(None); // clicked empty space → dismiss the panel
            return;
        }
        let hex = world.hex.clone();
        // If we already have the full sector cached, upgrade the world up front.
        let full_world = full_sectors
            .with_value(|fs| fs.get(&sector_coord).and_then(|s| s.worlds.iter().find(|w| w.hex == hex).cloned()));
        let already_full = full_world.is_some();
        selected.set(Some(SelectedWorld {
            world: full_world.unwrap_or(world),
            sector_name: sector_name.clone(),
            sector_coord,
            subsector,
            full: already_full,
        }));
        if already_full {
            return;
        }
        // Fetch the clicked sector at full LOD, then upgrade the selected world in
        // place (only if the user hasn't since clicked elsewhere).
        let encoded = String::from(js_sys::encode_uri_component(&sector_name));
        let m = milieu.get_untracked();
        spawn_local(async move {
            let url = format!("/api/sector/{m}/{encoded}?lod=full");
            let Ok(full) = fetch_json::<SectorData>(&url).await else { return };
            if milieu.get_untracked() != m {
                return; // milieu switched mid-fetch — drop stale full data
            }
            if let Some(fw) = full.worlds.iter().find(|w| w.hex == hex).cloned() {
                selected.update(|cur| {
                    if let Some(c) = cur {
                        if c.sector_coord == sector_coord && c.world.hex == hex {
                            c.world = fw;
                            c.full = true;
                        }
                    }
                });
            }
            full_sectors.update_value(|fs| {
                fs.insert(sector_coord, full);
            });
        });
    };
    let on_up = move |ev: web_sys::MouseEvent| {
        let up = (ev.client_x() as f64, ev.client_y() as f64);
        let is_click = down_pos
            .get_untracked()
            .is_some_and(|(dx, dy)| (up.0 - dx).abs() < 4.0 && (up.1 - dy).abs() < 4.0);
        drag.set(None);
        down_pos.set(None);
        if !is_click {
            return;
        }
        // Route mode wins (it's the explicit modal toggle); otherwise a click
        // selects a world for the detail panel.
        if route_open.get_untracked() {
            fill_endpoint(up);
        } else {
            select_world(up);
        }
    };
    let on_leave = move |_: web_sys::MouseEvent| {
        drag.set(None);
        down_pos.set(None);
    };
    // Build the worldgen solar-system request from a world's T5 fields and open
    // the popup (spinner immediately, then the render or an error). Shared by the
    // desktop double-click and the mobile long-press. The service does the
    // seeding/parsing/render; it needs the full-LOD fields (stellar/pbg/worlds).
    #[cfg(feature = "callisto")]
    let launch_system = move |sector_name: &str, w: &World| {
        let enc = |s: &str| String::from(js_sys::encode_uri_component(s));
        let mut url = format!(
            "{SYSTEM_SERVICE}?sector={}&hex={}&name={}&uwp={}&pbg={}&stellar={}&scale=2.0",
            enc(sector_name), enc(&w.hex), enc(&w.name), enc(&w.uwp), enc(&w.pbg), enc(&w.stellar),
        );
        if let Some(n) = w.worlds {
            url.push_str(&format!("&worlds={n}"));
        }
        let name = if w.name.is_empty() { w.hex.clone() } else { w.name.clone() };
        let title = format!("{name} — {} {}", sector_name, w.hex);
        launch_render(
            system_view, sys_zoom, sys_pan, sys_elapsed, sys_timer, sys_gen, url, title,
        );
    };
    // Double-click a world → solar-system popup (Callisto, dev-only). The
    // preceding single-clicks already selected + upgraded the world to full LOD,
    // so reuse `selected` rather than re-hit-testing.
    #[cfg(feature = "callisto")]
    let on_dblclick = move |_ev: web_sys::MouseEvent| {
        if route_open.get_untracked() {
            return; // route-planning mode owns clicks
        }
        let Some(sw) = selected.get_untracked() else { return };
        launch_system(&sw.sector_name, &sw.world);
    };
    #[cfg(not(feature = "callisto"))]
    let on_dblclick = move |_ev: web_sys::MouseEvent| {};

    // "World Map" button (detail panel, callisto-only): render the selected main
    // world's surface map. Builds the `/api/world` request from the world's T5
    // fields and opens the same popup (spinner → image), `orbit` left to the
    // service default (3 = main world).
    #[cfg(feature = "callisto")]
    let on_world_map = move |()| {
        let Some(sw) = selected.get_untracked() else { return };
        let w = &sw.world;
        let enc = |s: &str| String::from(js_sys::encode_uri_component(s));
        let name = if w.name.is_empty() { w.hex.clone() } else { w.name.clone() };
        let url = format!(
            "{WORLD_SERVICE}?sector={}&hex={}&name={}&uwp={}&scale=2.0",
            enc(&sw.sector_name), enc(&w.hex), enc(&name), enc(&w.uwp),
        );
        let title = format!("{name} — {} {} · World Map", sw.sector_name, w.hex);
        launch_render(
            system_view, sys_zoom, sys_pan, sys_elapsed, sys_timer, sys_gen, url, title,
        );
    };
    #[cfg(not(feature = "callisto"))]
    let on_world_map = move |()| {};

    // Long-press a world (mobile) → solar-system popup. Unlike the double-click,
    // nothing has selected the world yet, so first hit-test it (reusing
    // `select_world`, which also opens the detail panel + kicks the full-LOD
    // fetch), then render. If the world is already full we render now; otherwise
    // fetch the full sector first (the solar-system render needs stellar/pbg/worlds,
    // which overview LOD omits) and render when it lands.
    #[cfg(feature = "callisto")]
    let open_system_at = move |px: (f64, f64)| {
        if route_open.get_untracked() {
            return; // route-planning mode owns taps
        }
        select_world(px); // opens the panel + (if needed) starts the full fetch
        let Some(sw) = selected.get_untracked() else { return };
        if sw.full {
            launch_system(&sw.sector_name, &sw.world);
            return;
        }
        // Not cached at full LOD yet — fetch it ourselves, then render. (This may
        // duplicate the fetch select_world just kicked; an idempotent GET, so the
        // extra request is harmless for this dev-only feature.)
        let sector_name = sw.sector_name.clone();
        let sector_coord = sw.sector_coord;
        let hex = sw.world.hex.clone();
        let encoded = String::from(js_sys::encode_uri_component(&sector_name));
        let m = milieu.get_untracked();
        spawn_local(async move {
            let url = format!("/api/sector/{m}/{encoded}?lod=full");
            let Ok(full) = fetch_json::<SectorData>(&url).await else { return };
            if milieu.get_untracked() != m {
                return; // milieu switched mid-fetch
            }
            let Some(fw) = full.worlds.iter().find(|w| w.hex == hex).cloned() else { return };
            full_sectors.update_value(|fs| {
                fs.insert(sector_coord, full);
            });
            launch_system(&sector_name, &fw);
        });
    };

    // --- search ---
    let on_search = move |ev: web_sys::Event| {
        let q = event_target_value(&ev);
        if q.trim().is_empty() {
            set_results.set(Vec::new());
            return;
        }
        let encoded = String::from(js_sys::encode_uri_component(&q));
        let m = milieu.get_untracked();
        spawn_local(async move {
            if let Ok(r) = fetch_json::<SearchResults>(&format!("/api/search?q={encoded}&milieu={m}")).await {
                set_results.set(r.results.items.into_iter().map(Hit::from_item).collect());
            }
        });
    };
    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let Some(canvas_el) = canvas_ref.get_untracked() else {
            return;
        };
        let (w, h) = logical_dims(&canvas_el);
        let Some(v) = view.get_untracked() else {
            return;
        };
        let cursor = (ev.offset_x() as f64, ev.offset_y() as f64);
        let anchor = v.to_parsec(w, h, cursor);
        let factor = if ev.delta_y() < 0.0 { 1.1 } else { 1.0 / 1.1 };
        let scale = (v.scale * factor).clamp(render::MIN_SCALE, render::MAX_SCALE);
        let center = (
            anchor.0 - (cursor.0 - w / 2.0) / scale,
            anchor.1 - (cursor.1 - h / 2.0) / scale,
        );
        set_view.set(Some(ViewState { scale, center }));
    };

    // --- touch input: one finger pans, two fingers pinch-zoom. The canvas sets
    //     `touch-action:none`, so the browser does no default scrolling/zooming
    //     and these handlers fully own the gesture; tap-to-select still rides the
    //     synthesized mouse path (iOS only fakes mouse events for stationary
    //     taps, never for drags/pinches). ---
    let touch = RwSignal::new(None::<TouchGesture>);
    // Long-press state (callisto): a pending timer handle + the finger's origin.
    // A single stationary finger held LONG_PRESS_MS opens the solar-system view
    // (the mobile counterpart of the desktop double-click); any pan, second
    // finger, or lift cancels it.
    #[cfg(feature = "callisto")]
    let lp_timer = RwSignal::new(None::<i32>);
    #[cfg(feature = "callisto")]
    let lp_origin = RwSignal::new((0.0_f64, 0.0_f64));
    #[cfg(feature = "callisto")]
    let cancel_long_press = move || {
        if let Some(id) = lp_timer.get_untracked() {
            clear_timeout(id);
            lp_timer.set(None);
        }
    };
    // Seed (or re-seed) the gesture from whatever fingers are down. Used on
    // touchstart and after a finger lifts, so a pinch→pan handoff doesn't jump.
    let seed_touch = move |pts: &[(f64, f64)]| match *pts {
        [a, b, ..] => touch.set(Some(TouchGesture::Two { dist: pt_dist(a, b), mid: pt_mid(a, b) })),
        [a] => touch.set(Some(TouchGesture::One { last: a })),
        _ => touch.set(None),
    };
    let on_touch_start = move |ev: web_sys::TouchEvent| {
        let pts = touch_points(&ev);
        seed_touch(&pts);
        // Arm a long-press only for a single fresh finger outside route mode.
        #[cfg(feature = "callisto")]
        {
            cancel_long_press();
            if let [a] = *pts.as_slice() {
                if !route_open.get_untracked() {
                    lp_origin.set(a);
                    let id = set_timeout(LONG_PRESS_MS, move || {
                        lp_timer.set(None);
                        open_system_at(lp_origin.get_untracked());
                    });
                    lp_timer.set(Some(id));
                }
            }
        }
    };
    let on_touch_end = move |ev: web_sys::TouchEvent| {
        #[cfg(feature = "callisto")]
        cancel_long_press(); // a lift before the timer = a tap, not a long-press
        seed_touch(&touch_points(&ev));
    };
    let on_touch_move = move |ev: web_sys::TouchEvent| {
        let Some(canvas_el) = canvas_ref.get_untracked() else { return };
        let (w, h) = logical_dims(&canvas_el);
        let Some(v) = view.get_untracked() else { return };
        match *touch_points(&ev).as_slice() {
            // Two fingers: pan + zoom together — the parsec point under the prior
            // midpoint is moved under the current one, scaled by the pinch ratio.
            [a, b, ..] => {
                #[cfg(feature = "callisto")]
                cancel_long_press(); // a second finger means pinch, not long-press
                let (cur_d, cur_m) = (pt_dist(a, b), pt_mid(a, b));
                if let Some(TouchGesture::Two { dist: prev_d, mid: prev_m }) = touch.get_untracked() {
                    if prev_d > 0.0 {
                        let scale =
                            (v.scale * cur_d / prev_d).clamp(render::MIN_SCALE, render::MAX_SCALE);
                        let anchor = v.to_parsec(w, h, prev_m);
                        let center = (
                            anchor.0 - (cur_m.0 - w / 2.0) / scale,
                            anchor.1 - (cur_m.1 - h / 2.0) / scale,
                        );
                        set_view.set(Some(ViewState { scale, center }));
                    }
                }
                touch.set(Some(TouchGesture::Two { dist: cur_d, mid: cur_m }));
            }
            // One finger: drag-pan (mirrors the mouse `on_move` drag branch).
            [a] => {
                // Drifting past the slop means the user is panning, not pressing.
                #[cfg(feature = "callisto")]
                if pt_dist(a, lp_origin.get_untracked()) > LONG_PRESS_SLOP {
                    cancel_long_press();
                }
                if let Some(TouchGesture::One { last }) = touch.get_untracked() {
                    set_view.set(Some(ViewState {
                        center: (
                            v.center.0 - (a.0 - last.0) / v.scale,
                            v.center.1 - (a.1 - last.1) / v.scale,
                        ),
                        ..v
                    }));
                }
                touch.set(Some(TouchGesture::One { last: a }));
            }
            _ => {}
        }
    };

    // Compute the jump route at jump rating `j` from the two planner endpoints
    // and draw it. Called by the J-1…J-6 pills (each picks its rating). `Copy`
    // (captures only signals/stores), so it's reused across all six buttons.
    let do_route = move |j: i32| {
        route_jump.set(j);
        let (s, e) = (route_start.get_untracked(), route_end.get_untracked());
        if s.trim().is_empty() || e.trim().is_empty() {
            set_route_status.set("Set start & destination — type a world name or click the map.".into());
            return;
        }
        set_route_status.set("Computing route…".into());
        let m = milieu.get_untracked();
        let opts = (
            route_wild.get_untracked(),
            route_im.get_untracked(),
            route_nored.get_untracked(),
            route_aok.get_untracked(),
        );
        spawn_local(async move {
            match fetch_route(&s, &e, j, m, opts).await {
                Ok(r) => {
                    // Summary + waypoint list render from `route` itself; clear
                    // the transient status line.
                    set_route_status.set(String::new());
                    // Center the view on the route's start so it's visible.
                    if let Some(wp) = r.waypoints.first() {
                        set_view.set(Some(ViewState {
                            scale: 32.0,
                            center: render::world_to_parsec(wp.coord.x, wp.coord.y),
                        }));
                    }
                    route.set(Some(r));
                }
                Err(err) => {
                    route.set(None);
                    set_route_status.set(format!("No route: {err}"));
                }
            }
        });
    };
    // Clear the planner (✕): wipe endpoints + the drawn route.
    let clear_route = move || {
        route_start.set(String::new());
        route_end.set(String::new());
        route_jump.set(0);
        set_route_status.set(String::new());
        route.set(None);
    };
    // Swap start ⇄ destination (⇅) and recompute if a jump is already chosen.
    let swap_route = move || {
        let (s, e) = (route_start.get_untracked(), route_end.get_untracked());
        route_start.set(e);
        route_end.set(s);
        let j = route_jump.get_untracked();
        if j > 0 {
            do_route(j);
        }
    };
    // One route-option checkbox: flips its flag and (if a route is already
    // computed) recomputes with the new constraint, like the reference.
    let route_opt = move |sig: RwSignal<bool>, label: &'static str, title: &'static str| {
        view! {
            <label title=title
                   style="display:flex; align-items:center; gap:7px; cursor:pointer; \
                          padding:2px 0; font:12px system-ui,sans-serif; color:#333;">
                <input type="checkbox" prop:checked=move || sig.get()
                       on:change=move |ev| {
                           sig.set(event_target_checked(&ev));
                           let j = route_jump.get_untracked();
                           if j > 0 { do_route(j); }
                       }
                       style="width:14px; height:14px; accent-color:#e32736; cursor:pointer; flex:none;" />
                <span>{label}</span>
            </label>
        }
    };
    // Copy the computed route to the clipboard — matches the reference
    // `RouteResultsTextTemplate`: "<D> parsecs -- <J> jumps", then per stop
    // "* Name (Sector Hex)" with "Jump <d> to" between consecutive stops.
    let do_copy = move |_: web_sys::MouseEvent| {
        let text = route.with(|r| {
            r.as_ref()
                .map(|r| {
                    let wps = &r.waypoints;
                    let mut s = format!("{} parsecs -- {} jumps\n\n", r.parsecs, r.jumps);
                    for (i, w) in wps.iter().enumerate() {
                        s.push_str(&format!("* {} ({} {})\n", w.name, w.sector, w.hex));
                        if i + 1 < wps.len() {
                            let leg = w.coord.hex_distance(wps[i + 1].coord);
                            s.push_str(&format!("\n    Jump {leg} to\n\n"));
                        }
                    }
                    s
                })
                .unwrap_or_default()
        });
        if !text.is_empty() {
            let _ = win().navigator().clipboard().write_text(&text);
        }
    };
    // Print just the route: open a window with a formatted route sheet (mirrors
    // the reference print/route.html) and let it print itself.
    let do_print = move |_: web_sys::MouseEvent| {
        let html = route.with(|r| r.as_ref().map(|r| route_print::build_route_print_html(r, route_jump.get_untracked())));
        if let Some(html) = html {
            open_print_html(&html);
        }
    };

    // Home → the charted-space overview.
    let on_home = move |_: web_sys::MouseEvent| {
        if let Some(cv) = canvas_ref.get_untracked() {
            let (lw, lh) = logical_dims(&cv);
            set_view.set(Some(render::home_view(lw, lh)));
        }
        panel.set(0);
    };

    // Callisto solar-system popup (dev-only). Built as an `AnyView` so the main
    // `view!` embeds `{system_modal}` unconditionally; without the feature it's
    // nothing.
    #[cfg(feature = "callisto")]
    let close_system = move || {
        if let Some(ImgView::Ready { obj, .. }) = system_view.get_untracked() {
            let _ = web_sys::Url::revoke_object_url(&obj);
        }
        if let Some(id) = sys_timer.get_untracked() {
            win().clear_interval_with_handle(id);
            sys_timer.set(None);
        }
        sys_gen.update(|g| *g = g.wrapping_add(1)); // ignore any in-flight fetch
        system_view.set(None);
    };
    #[cfg(feature = "callisto")]
    let reset_sys = move || {
        sys_zoom.set(1.0);
        sys_pan.set((0.0, 0.0));
    };
    // Wheel zoom, anchored on the cursor so the point under it stays put.
    #[cfg(feature = "callisto")]
    let on_sys_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let z = sys_zoom.get_untracked();
        let nz = (z * if ev.delta_y() < 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(1.0, 8.0);
        if (nz - z).abs() < 1e-6 {
            return;
        }
        // Cursor position relative to the viewport's center (transform-origin).
        let (cx, cy) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .map(|el| {
                let r = el.get_bounding_client_rect();
                (ev.client_x() as f64 - (r.x() + r.width() / 2.0), ev.client_y() as f64 - (r.y() + r.height() / 2.0))
            })
            .unwrap_or((0.0, 0.0));
        if nz <= 1.0 + 1e-6 {
            sys_pan.set((0.0, 0.0)); // fully out → recenter
        } else {
            let (px, py) = sys_pan.get_untracked();
            sys_pan.set((cx - nz * (cx - px) / z, cy - nz * (cy - py) / z));
        }
        sys_zoom.set(nz);
    };
    #[cfg(feature = "callisto")]
    let on_sys_down = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        let (px, py) = sys_pan.get_untracked();
        sys_drag.set(Some((ev.client_x() as f64, ev.client_y() as f64, px, py)));
    };
    #[cfg(feature = "callisto")]
    let on_sys_move = move |ev: web_sys::MouseEvent| {
        if let Some((sx, sy, opx, opy)) = sys_drag.get_untracked() {
            sys_pan.set((opx + ev.client_x() as f64 - sx, opy + ev.client_y() as f64 - sy));
        }
    };
    #[cfg(feature = "callisto")]
    let on_sys_up = move |_ev: web_sys::MouseEvent| sys_drag.set(None);

    // A full-screen "zoom into the world" overlay: the high-res system render in
    // a zoom (wheel) / pan (drag) viewport, with Reset / Print / Download / Close.
    #[cfg(feature = "callisto")]
    let btn = "padding:6px 12px; border-radius:15px; cursor:pointer; border:1px solid #2a3145; \
               background:rgba(40,44,58,0.7); color:#cdd5e6; font:600 12px system-ui;";
    #[cfg(feature = "callisto")]
    let system_modal = view! {
        <Show when=move || system_view.get().is_some()>
            <div style="position:fixed; inset:0; z-index:30; display:flex; flex-direction:column; \
                        background:rgba(0,0,0,0.9);">
                {move || match system_view.get() {
                    // Render in flight — spinner + reassuring copy + live elapsed counter.
                    Some(ImgView::Loading { title }) => view! {
                        <style>"@keyframes tmap-spin{to{transform:rotate(360deg)}}"</style>
                        <div style="flex:1; display:flex; flex-direction:column; align-items:center; \
                                    justify-content:center; gap:20px; text-align:center; padding:24px;">
                            <div style="width:66px; height:66px; border:6px solid rgba(255,255,255,0.14); \
                                        border-top-color:#e32736; border-radius:50%; \
                                        animation:tmap-spin 0.9s linear infinite;"></div>
                            <div style="font:700 17px system-ui; color:#fff; max-width:32ch;">{title}</div>
                            <div style="font:13px system-ui; color:#9aa3b8; max-width:38ch; line-height:1.5;">
                                "Generating the map — the first render of a world can take up to a minute. \
                                 It's cached after that, so it'll be instant next time."
                            </div>
                            <div style="font:700 30px ui-monospace,monospace; color:#e9eef9; letter-spacing:0.04em;">
                                {move || format!("{}s", sys_elapsed.get())}
                            </div>
                            <button style=btn on:click=move |_| close_system()>"Cancel"</button>
                        </div>
                    }.into_any(),
                    // Generation failed — surface the service's reason (e.g. a 422 partial UWP).
                    Some(ImgView::Error { title, msg }) => view! {
                        <div style="flex:1; display:flex; flex-direction:column; align-items:center; \
                                    justify-content:center; gap:16px; text-align:center; padding:24px;">
                            <div style="font-size:40px; line-height:1;">"🛰"</div>
                            <div style="font:700 17px system-ui; color:#fff; max-width:32ch;">{title}</div>
                            <div style="font:14px system-ui; color:#e3a3a8; max-width:40ch; line-height:1.5;">{msg}</div>
                            <button style=btn on:click=move |_| close_system()>"Close"</button>
                        </div>
                    }.into_any(),
                    // Rendered — the zoom (wheel) / pan (drag) viewer with Reset / Print / Download.
                    Some(ImgView::Ready { obj, svc, title }) => {
                        let (svc_p, title_p) = (svc.clone(), title.clone());
                        let (obj_d, title_d) = (obj.clone(), title.clone());
                        view! {
                            <div style="flex:none; display:flex; align-items:center; gap:8px; padding:10px 14px;">
                                <span style="flex:1; min-width:0; font:700 15px system-ui; color:#fff; \
                                             overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">
                                    {title.clone()}
                                </span>
                                <button style=btn on:click=move |_| reset_sys()>"⟳  Reset"</button>
                                // Print uses the absolute service URL (a blob: print doc can't resolve our object URL).
                                <button style=btn on:click=move |_| print_image_url(&svc_p, &title_p)>"🖨  Print"</button>
                                // Download uses the object URL (cross-origin download attr is ignored otherwise).
                                <button style=btn on:click=move |_| download_url(&obj_d, &format!("{title_d}.png"))>"⬇  Download"</button>
                                <span on:click=move |_| close_system()
                                      style="cursor:pointer; color:#fff; font-size:24px; line-height:1; padding:0 6px;">"✕"</span>
                            </div>
                            <div on:wheel=on_sys_wheel on:mousedown=on_sys_down on:mousemove=on_sys_move
                                 on:mouseup=on_sys_up on:mouseleave=on_sys_up
                                 style="flex:1; overflow:hidden; display:flex; align-items:center; justify-content:center; \
                                        touch-action:none;"
                                 style:cursor=move || if sys_drag.get().is_some() { "grabbing" } else { "grab" }>
                                <img src=obj draggable="false"
                                     style="max-width:98%; max-height:98%; transform-origin:center center; \
                                            user-select:none; -webkit-user-drag:none;"
                                     style:transform=move || {
                                         let z = sys_zoom.get();
                                         let (px, py) = sys_pan.get();
                                         format!("translate({px}px, {py}px) scale({z})")
                                     } />
                            </div>
                        }.into_any()
                    }
                    None => ().into_any(),
                }}
            </div>
        </Show>
    }
    .into_any();
    #[cfg(not(feature = "callisto"))]
    let system_modal = ().into_any();

    // --- Share panel helpers: a read-only field + Copy button, fed by memos off
    //     the live `share_url` (link as-is; embed wrapped in an <iframe>). ---
    let copy_to_clipboard = move |text: String| {
        let _ = win().navigator().clipboard().write_text(&text);
    };
    let link_value = Memo::new(move |_| share_url.get());
    let embed_value = Memo::new(move |_| {
        let u = share_url.get();
        if u.is_empty() {
            String::new()
        } else {
            format!("<iframe width=400 height=300 src=\"{u}\">")
        }
    });
    let share_field = move |label: &'static str, value: Memo<String>| {
        view! {
            <div style="margin-bottom:10px;">
                <div style="color:#cfd6e6; font-size:12px; margin-bottom:4px;">{label}</div>
                <div style="display:flex; gap:6px;">
                    <input prop:value=move || value.get() readonly=true
                           style="flex:1; min-width:0; padding:6px 8px; border-radius:6px; \
                                  border:1px solid #2a3145; background:#0c0f18; color:#cfd6e6; \
                                  font:12px ui-monospace,monospace;" />
                    <button on:click=move |_| copy_to_clipboard(value.get_untracked())
                            style="flex:none; padding:6px 12px; border-radius:6px; cursor:pointer; \
                                   border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                   color:#cdd5e6; font:600 12px system-ui;">"Copy"</button>
                </div>
            </div>
        }
    };

    view! {
        <main style="margin:0; padding:0; overflow:hidden; background:#000;">
            <canvas node_ref=canvas_ref
                    style="position:fixed; top:0; left:0; width:100vw; height:100dvh; \
                           display:block; touch-action:none; -webkit-touch-callout:none; \
                           -webkit-user-select:none; user-select:none;"
                    style:cursor=move || {
                        if route_open.get() { "crosshair" }
                        else if drag.get().is_some() { "grabbing" }
                        else if hover_world.get() { "default" } // over a world (callisto) → clickable
                        else { "grab" }
                    }
                    on:mousedown=on_down
                    on:mousemove=on_move
                    on:mouseup=on_up
                    on:mouseleave=on_leave
                    on:dblclick=on_dblclick
                    on:wheel=on_wheel
                    on:touchstart=on_touch_start
                    on:touchmove=on_touch_move
                    on:touchend=on_touch_end
                    on:touchcancel=on_touch_end></canvas>
            <div style="position:fixed; top:10px; left:12px; width:320px; \
                        font:14px system-ui,sans-serif; color:#cfd6e6;">
                <div style="display:flex; gap:6px; align-items:stretch;">
                    <div style="flex:1; min-width:0; position:relative;">
                        <input type="search" placeholder="Search…"
                               on:input=on_search
                               style="width:100%; box-sizing:border-box; padding:8px 32px 8px 12px; \
                                      border-radius:6px; border:1px solid #c5ccd8; \
                                      background:#fff; color:#222; font-size:15px; outline:none;" />
                        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="#888"
                             stroke-width="2" stroke-linecap="round"
                             style="position:absolute; right:9px; top:50%; transform:translateY(-50%); pointer-events:none;">
                            <circle cx="11" cy="11" r="7"></circle>
                            <path d="M21 21 L16 16"></path>
                        </svg>
                    </div>
                    <button title="Jump route"
                            on:click=move |_| route_open.update(|o| *o = !*o)
                            on:mouseenter=move |_| route_hover.set(true)
                            on:mouseleave=move |_| route_hover.set(false)
                            style:background=move || if route_open.get() { "#e32736" } else { "rgba(40,44,58,0.92)" }
                            style:color=move || {
                                if route_open.get() { "#fff" }
                                else if route_hover.get() { "#e32736" }
                                else { "#cdd5e6" }
                            }
                            style="flex:none; width:40px; border:1px solid #2a3145; border-radius:6px; \
                                   cursor:pointer; display:flex; align-items:center; justify-content:center;">
                        <svg width="22" height="22" viewBox="0 0 24 24" fill="none"
                             stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <circle cx="5" cy="18" r="2.4" fill="currentColor" stroke="none"></circle>
                            <circle cx="19" cy="6" r="2.4" fill="currentColor" stroke="none"></circle>
                            <path d="M5 18 L10 11 L14 14 L19 6"></path>
                        </svg>
                    </button>
                </div>
                <div style="margin-top:4px; max-height:60dvh; overflow:auto; \
                            background:rgba(10,12,20,0.92); border-radius:6px;">
                    <For each=move || results.get()
                         key=|r| format!("{}/{}", r.sector, r.hex.clone().unwrap_or_default())
                         let:r>
                        <div on:click=move |_| {
                                 set_view.set(Some(ViewState {
                                     scale: 64.0,
                                     center: render::world_to_parsec(r.coord.0, r.coord.1),
                                 }));
                                 set_results.set(Vec::new());
                             }
                             style="padding:6px 10px; cursor:pointer; \
                                    border-bottom:1px solid #1c2130;">
                            <span style="color:#e9eef9;">{r.name.clone()}</span>
                            <span style="color:#8a93a8;">
                                " · "{r.sector.clone()}
                                {r.hex.clone().map(|h| format!(" {h}")).unwrap_or_default()}
                            </span>
                        </div>
                    </For>
                </div>
                <div style="margin-top:6px; opacity:0.7; text-shadow:0 1px 3px #000;">
                    {move || status.get()}
                </div>

            </div>
            // --- jump-route planner panel (toggled by the route button) ---
            <Show when=move || route_open.get()>
                <div style="position:fixed; top:56px; left:12px; width:300px; \
                            max-width:calc(100vw - 24px); box-sizing:border-box; \
                            max-height:min(350px, calc(100dvh - 70px)); display:flex; flex-direction:column; \
                            padding:10px 14px 12px; border-radius:0; background:#fff; \
                            border:1px solid #000; box-shadow:0 6px 26px rgba(0,0,0,0.5); \
                            font:13px system-ui,sans-serif; color:#222;">
                    <div style="flex:none; display:flex; align-items:center; gap:6px; \
                                border-bottom:1px solid #d8d8d8; margin-bottom:8px;">
                        <input type="text" placeholder="Start (type or click map)"
                               bind:value=route_start
                               style="flex:1; min-width:0; border:none; outline:none; \
                                      background:transparent; color:#222; \
                                      font:16px system-ui,sans-serif; padding:7px 2px;" />
                        <button title="Clear" tabindex="-1" on:click=move |_| clear_route()
                                style="border:none; background:transparent; cursor:pointer; \
                                       font-size:18px; color:#888; padding:0 4px; line-height:1;">"✕"</button>
                    </div>
                    <div style="flex:none; display:flex; align-items:center; gap:6px; \
                                border-bottom:1px solid #d8d8d8; margin-bottom:10px;">
                        <input type="text" placeholder="Destination (type or click map)"
                               bind:value=route_end
                               style="flex:1; min-width:0; border:none; outline:none; \
                                      background:transparent; color:#222; \
                                      font:16px system-ui,sans-serif; padding:7px 2px;" />
                        <button title="Swap start & destination" tabindex="-1" on:click=move |_| swap_route()
                                style="border:none; background:transparent; cursor:pointer; \
                                       font-size:16px; color:#444; padding:0 4px; line-height:1;">"⇅"</button>
                    </div>
                    <div style="flex:none; display:flex; gap:5px;">
                        {[("J-1", 1), ("J-2", 2), ("J-3", 3), ("J-4", 4), ("J-5", 5), ("J-6", 6), ("H-1", 10)]
                            .into_iter().map(|(label, n)| view! {
                                <button on:click=move |_| do_route(n)
                                        title=move || if n == 10 { "Calculate a Hop-1 (10 pc) route" } else { "" }
                                        style:background=move || if route_jump.get() == n { "#e32736" } else { "#fff" }
                                        style:color=move || if route_jump.get() == n { "#fff" } else { "#333" }
                                        style="flex:1; padding:0; line-height:28px; border:1px solid #ccc; \
                                               border-radius:14px; cursor:pointer; \
                                               font:600 12px system-ui,sans-serif;">
                                    {label}
                                </button>
                            }).collect_view()}
                    </div>
                    // Route-finding constraints (the reference's routeOptions).
                    <div style="flex:none; margin-top:8px;">
                        {route_opt(route_wild, "Require Wilderness Fueling", "Stops must have a gas giant or liquid water present")}
                        {route_opt(route_im, "Only Imperial Worlds", "Stops must be member worlds of the Third Imperium")}
                        {route_opt(route_nored, "Avoid Red Zones", "Stops must not be TAS Red Zone (restricted)")}
                        {route_opt(route_aok, "Allow Anomalies / Calibration Points", "Allow stops at deep-space stations, calibration points, and other anomalies")}
                    </div>
                    <div style="flex:none; min-height:1em; color:#555; font-size:12px; \
                                text-align:center; margin-top:6px;">
                        {move || route_status.get()}
                    </div>
                    // Scrollable results region — the box is capped, the list scrolls.
                    <div style="flex:1; min-height:0; overflow:auto; margin-top:4px;">
                        {move || route.with(|r| r.as_ref().map(|r| {
                            let wps = &r.waypoints;
                            let summary = format!("{} parsecs — {} jumps", r.parsecs, r.jumps);
                            let rows = wps.iter().enumerate().map(|(i, w)| {
                                let leg = (i > 0).then(|| wps[i - 1].coord.hex_distance(w.coord));
                                let (name, sub) = (w.name.clone(), format!("{} {}", w.sector, w.hex));
                                view! {
                                    <div>
                                        {leg.map(|d| view! {
                                            <div style="display:flex; height:20px;">
                                                <div style="width:26px; flex:none; position:relative; \
                                                            display:flex; align-items:center; justify-content:center;">
                                                    <div style="position:absolute; top:-3px; bottom:-3px; left:50%; \
                                                                transform:translateX(-50%); width:3px; background:#2e7d2e;"></div>
                                                    <span style="position:relative; background:#fff; padding:0 2px; \
                                                                 font:600 13px system-ui; color:#222;">{d}</span>
                                                </div>
                                            </div>
                                        })}
                                        <div style="display:flex; align-items:center;">
                                            <div style="width:26px; flex:none; display:flex; justify-content:center;">
                                                <div style="width:13px; height:13px; border-radius:50%; \
                                                            background:#2e7d2e;"></div>
                                            </div>
                                            <div style="flex:1; min-width:0;">
                                                <div style="font:600 16px system-ui; color:#111; \
                                                            text-decoration:underline; line-height:1.1;">{name}</div>
                                                <div style="font:12px system-ui; color:#666;">{sub}</div>
                                            </div>
                                        </div>
                                    </div>
                                }
                            }).collect_view();
                            view! {
                                <div style="text-align:center; font:600 14px system-ui; color:#222; \
                                            padding:5px 0 10px;">{summary}</div>
                                <div>{rows}</div>
                                <div style="display:flex; gap:10px; margin-top:14px; \
                                            border-top:1px solid #e3e3e3; padding-top:12px;">
                                    <button on:click=do_print
                                            style="flex:1; padding:7px 0; border:1px solid #ccc; border-radius:16px; \
                                                   background:#fff; color:#333; cursor:pointer; font:600 12px system-ui;">
                                        "🖨  Print"</button>
                                    <button on:click=do_copy
                                            style="flex:1; padding:7px 0; border:1px solid #ccc; border-radius:16px; \
                                                   background:#fff; color:#333; cursor:pointer; font:600 12px system-ui;">
                                        "⧉  Copy"</button>
                                </div>
                            }
                        }))}
                    </div>
                </div>
            </Show>
            // --- world detail panel (click a world → its T5 data sheet) ---
            <WorldPanel
                selected=selected
                on_close=move |()| selected.set(None)
                on_plan_route=move |label: String| {
                    route_start.set(label);
                    route_end.set(String::new());
                    route.set(None);
                    route_open.set(true);
                    selected.set(None);
                }
                on_print=move |()| {
                    if let Some(sel) = selected.get_untracked() {
                        open_print_html(&world_print::build_world_print_html(&sel));
                    }
                }
                on_world_map=on_world_map
                on_jump_range=move |n: i32| {
                    let Some(sel) = selected.get_untracked() else { return };
                    let id = (sel.sector_coord, sel.world.hex.clone());
                    // Toggle: clicking the active J-N again closes the cutout.
                    let active = jumpmap
                        .get_untracked()
                        .filter(|(c, h, _, _)| (*c, h.clone()) == id)
                        .map(|(_, _, _, j)| j);
                    if active == Some(n) {
                        jumpmap.set(None);
                        return;
                    }
                    // Build the origin's absolute world hex `Coord` the same way
                    // the route planner does (`world_hex(sx, sy, col, row)`), so
                    // the neighborhood matches route distances.
                    let Some((col, row)) = parse_hex(&sel.world.hex) else { return };
                    let (wc, wr) = render::world_hex(sel.sector_coord.0, sel.sector_coord.1, col, row);
                    let name = if sel.world.name.is_empty() {
                        format!("{} {}", sel.sector_name, sel.world.hex)
                    } else {
                        sel.world.name.clone()
                    };
                    jumpmap.set(Some((
                        sel.sector_coord,
                        name,
                        tmap_core::astrometrics::Coord::new(wc, wr),
                        n,
                    )));
                }
                active_jump=Signal::derive(move || {
                    let sel = selected.get();
                    jumpmap
                        .get()
                        .filter(|(c, h, _, _)| {
                            sel.as_ref().is_some_and(|s| s.sector_coord == *c && &s.world.hex == h)
                        })
                        .map(|(_, _, _, j)| j)
                        .unwrap_or(0)
                }) />
            // --- jump-N neighborhood cutout overlay (a J-N pill renders it) ---
            <Show when=move || jumpmap.get().is_some()>
                <div style="position:fixed; top:56px; left:50%; transform:translateX(-50%); \
                            box-sizing:border-box; padding:12px 14px 14px; border-radius:8px; \
                            background:rgba(12,15,24,0.97); border:1px solid #2a3145; \
                            box-shadow:0 6px 26px rgba(0,0,0,0.6); z-index:25; \
                            font:13px system-ui,sans-serif; color:#cfd6e6;">
                    <div style="display:flex; align-items:center; gap:10px; margin-bottom:8px;">
                        <span style="flex:1; font:700 14px system-ui; color:#fff;">
                            {move || jumpmap.get().map(|(_, n, _, j)| format!("Jump-{j} Neighborhood — {n}")).unwrap_or_default()}
                        </span>
                        <span on:click=move |_| jumpmap.set(None)
                              style="cursor:pointer; color:#8a93a8; font-size:18px; line-height:1;">"✕"</span>
                    </div>
                    <canvas node_ref=jumpmap_ref width="360" height="360"
                            style="display:block; width:360px; height:360px; \
                                   max-width:calc(100vw - 60px); border-radius:4px; background:#e8e8e8;"></canvas>
                    <div style="display:flex; gap:8px; margin-top:10px;">
                        <button on:click=move |_| {
                                    if let Some(cv) = jumpmap_ref.get_untracked() {
                                        let title = jumpmap.get_untracked()
                                            .map(|(_, n, _, j)| format!("Jump-{j} Neighborhood: {n}")).unwrap_or_default();
                                        print_canvas(&cv, &title);
                                    }
                                }
                                style="flex:1; padding:7px 0; border-radius:15px; cursor:pointer; \
                                       border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                       color:#cdd5e6; font:600 12px system-ui;">"🖨  Print"</button>
                        <button on:click=move |_| {
                                    if let Some(cv) = jumpmap_ref.get_untracked() {
                                        let fname = jumpmap.get_untracked()
                                            .map(|(_, n, _, j)| format!("{n} jump-{j}.png")).unwrap_or_else(|| "jumpmap.png".into());
                                        download_canvas_png(&cv, &fname);
                                    }
                                }
                                style="flex:1; padding:7px 0; border-radius:15px; cursor:pointer; \
                                       border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                       color:#cdd5e6; font:600 12px system-ui;">"⬇  Download PNG"</button>
                    </div>
                </div>
            </Show>
            {system_modal}
            // --- bottom pane (mirrors the reference #bottom-pane): red stripe,
            //     the per-sector data-source credit (or Mongoose copyright) on the
            //     left, the TRAVELLER® wordmark on the right. ---
            <div style="position:fixed; left:0; right:0; bottom:0; box-sizing:border-box; \
                        overflow:hidden; pointer-events:none; color:#fff; \
                        background:rgba(0,0,0,0.6); font:14px Helvetica,Arial,sans-serif;">
                <div style="height:7px; background:#e32736;"></div>
                <div style="display:flex; align-items:flex-start; justify-content:space-between; \
                            gap:18px; padding:8px 14px 10px;">
                    <div style="flex:1; min-width:0; font-style:italic; line-height:1.45; \
                                text-shadow:0 1px 2px #000; overflow:hidden; \
                                display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:2;">
                        {move || {
                            let c = footer_credit.get();
                            if c.is_empty() {
                                view! {
                                    <span>"The "<b><i>"Traveller"</i></b>" game in all forms is owned by \
                                        Mongoose Publishing. Copyright 1977 \u{2013} 2024 Mongoose Publishing."</span>
                                }.into_any()
                            } else {
                                view! { <span>{c}</span> }.into_any()
                            }
                        }}
                    </div>
                    <div style="flex:none; width:300px; height:51px; \
                                background:url('/api/res/ui/logo-flat.svg') no-repeat top right; \
                                background-size:contain;"></div>
                </div>
            </div>

            // --- top-right control cluster: home / clock / key / hamburger ---
            <div style="position:fixed; top:10px; right:12px; display:flex; gap:6px;">
                <button on:click=on_home title="Home — charted-space overview"
                        style=BTN_STYLE>"⌂"</button>
                <button title="Milieu — time period" style=BTN_STYLE
                        on:click=move |_| panel.update(|p| *p = if *p == 3 { 0 } else { 3 })>
                    <svg viewBox="0 0 24 24" width="19" height="19" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" style="vertical-align:middle;">
                        <circle cx="12" cy="12" r="9"></circle>
                        <path d="M12 7 V12 L15.5 14"></path>
                    </svg>
                </button>
                <button title="Map key / legend" style=BTN_STYLE
                        on:click=move |_| panel.update(|p| *p = if *p == 1 { 0 } else { 1 })>
                    <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor"
                         style="vertical-align:middle;">
                        <path d="M21 10h-8.35A5.99 5.99 0 0 0 7 6c-3.31 0-6 2.69-6 6s2.69 6 6 \
                                 6a5.99 5.99 0 0 0 5.65-4H13l2 2 2-2 2 2 4-4.04L21 10zM7 15c-1.65 \
                                 0-3-1.35-3-3s1.35-3 3-3 3 1.35 3 3-1.35 3-3 3z" />
                    </svg>
                </button>
                <button title="Settings & layers" style=BTN_STYLE
                        on:click=move |_| panel.update(|p| *p = if *p == 2 { 0 } else { 2 })>"☰"</button>
                <button title="Share / embed this view" style=BTN_STYLE
                        on:click=move |_| panel.update(|p| *p = if *p == 4 { 0 } else { 4 })>
                    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         style="vertical-align:middle;">
                        <circle cx="18" cy="5" r="3"></circle>
                        <circle cx="6" cy="12" r="3"></circle>
                        <circle cx="18" cy="19" r="3"></circle>
                        <path d="M8.6 13.5 L15.4 17.5 M15.4 6.5 L8.6 10.5"></path>
                    </svg>
                </button>
            </div>

            // --- milieu / time selector panel ---
            <Show when=move || panel.get() == 3>
                <div style=PANEL_STYLE>
                    <div style="display:flex; justify-content:space-between; align-items:center;">
                        <span style="font-weight:700; letter-spacing:0.05em;">"MILIEU"</span>
                        <span on:click=move |_| panel.set(0)
                              style="cursor:pointer; color:#8a93a8; font-size:18px;">"✕"</span>
                    </div>
                    <hr style="border:none; border-top:1px solid #2a3145; margin:8px 0 6px;" />
                    <div style="color:#8a93a8; font-size:12px; margin-bottom:6px;">
                        "Era snapshot of charted space. Switching reloads sector data."
                    </div>
                    {MILIEUX.iter().map(|(code, label)| {
                        let code = *code;
                        let label = *label;
                        view! {
                            <div on:click=move |_| { milieu.set(code); panel.set(0); }
                                 style="display:flex; align-items:baseline; gap:10px; padding:7px 4px; \
                                        cursor:pointer; border-bottom:1px solid #20283a;"
                                 style:color=move || if milieu.get() == code { "#e32736" } else { "#dfe5f2" }>
                                <span style="flex:none; width:14px; text-align:center; font-weight:700;">
                                    {move || if milieu.get() == code { "●" } else { "" }}
                                </span>
                                <span style="flex:none; width:46px; font-weight:700; font-size:12px;">{code}</span>
                                <span>{label}</span>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </Show>

            // --- share panel: permalink + embed code for the current view ---
            <Show when=move || panel.get() == 4>
                <div style=PANEL_STYLE>
                    <div style="display:flex; justify-content:space-between; align-items:center;">
                        <span style="font-weight:700; letter-spacing:0.05em;">"SHARE"</span>
                        <span on:click=move |_| panel.set(0)
                              style="cursor:pointer; color:#8a93a8; font-size:18px;">"✕"</span>
                    </div>
                    <hr style="border:none; border-top:1px solid #2a3145; margin:8px 0 8px;" />
                    <div style="color:#8a93a8; font-size:12px; margin-bottom:8px;">
                        "A link to exactly what's on screen — position, zoom, and milieu. \
                         Opening it restores this view."
                    </div>
                    {share_field("Share this link", link_value)}
                    {share_field("Embed this HTML", embed_value)}
                </div>
            </Show>

            // --- legend / key panel (ported from index.html #legendBox) ---
            <Show when=move || panel.get() == 1>
                <div style=PANEL_STYLE>
                    <div style="display:flex; justify-content:space-between; align-items:center;">
                        <span style="font-weight:700; letter-spacing:0.05em;">"MAP LEGEND"</span>
                        <span on:click=move |_| panel.set(0)
                              style="cursor:pointer; color:#8a93a8; font-size:18px;">"✕"</span>
                    </div>
                    <hr style="border:none; border-top:1px solid #2a3145; margin:8px 0 6px;" />
                    // The reference's two hex-diagram plates (poster theme SVGs).
                    <img src="/api/res/ui/Legend_1003_poster.svg"
                         style="width:190px; display:block; margin:2px auto;" />
                    <img src="/api/res/ui/Legend_1006_poster.svg"
                         style="width:190px; display:block; margin:2px auto 6px;" />

                    {section_header("WORLD CHARACTERISTICS")}
                    {swatch("#ffcc00", false, "Rich & Agricultural")}
                    {swatch("#048104", false, "Agricultural")}
                    {swatch("#a000a0", false, "Rich")}
                    {swatch("#888888", false, "Industrial")}
                    {swatch("#cc6626", false, "Corrosive / Insidious")}
                    {swatch("#000000", true, "Vacuum")}
                    {swatch("#00bfff", false, "Water Present")}
                    {swatch("#ffffff", false, "No Water Present")}
                    {legend_row(":::", "#cfd6e6", "Asteroid Belt")}
                    {legend_row("∗", "#cfd6e6", "Unknown")}
                    {legend_row("⌖", "#e8636f", "Anomaly")}

                    {section_header("BASES")}
                    {legend_row("★", "#e9eef9", "Imperial Naval Base")}
                    {legend_row("▲", "#e9eef9", "Imperial Scout Base")}
                    {legend_row("▲", "#e8636f", "Imperial Scout Way Station")}
                    {legend_row("■", "#e9eef9", "Imperial Naval Depot")}
                    {legend_row("◆", "#e9eef9", "Zhodani Base")}
                    {legend_row("◆", "#e8636f", "Zhodani Relay Station")}
                    {legend_row("★", "#e8636f", "Other Naval / Tlauku Base")}
                    {legend_row("■", "#e8636f", "Other Naval Outpost / Depot")}
                    {legend_row("∗∗", "#e9eef9", "Corsair / Clan / Embassy")}
                    {legend_row("✦", "#e9eef9", "Military Base / Garrison")}
                    {legend_row("•", "#e9eef9", "Independent Base")}
                    {legend_row("Γ", "#e8636f", "Research Station")}
                    {legend_row("R", "#e9eef9", "Imperial Reserve")}
                    {legend_row("P", "#e8636f", "Penal Colony")}
                    {legend_row("X", "#e9eef9", "Prison, Exile Camp")}

                    {section_header("TRAVEL ZONES")}
                    {legend_row("▬", "#ffcc00", "Amber Zone")}
                    {legend_row("▬", "#e32736", "Red Zone")}

                    {section_header("POPULATION")}
                    <div style="display:flex; gap:12px; padding:3px 0;">
                        <span style="width:54px; color:#9aa3b8;">"Wef"</span>
                        <span style="color:#dfe5f2;">"under 1 billion"</span>
                    </div>
                    <div style="display:flex; gap:12px; padding:3px 0;">
                        <span style="width:54px; color:#e9eef9; font-weight:700;">"YNAM"</span>
                        <span style="color:#dfe5f2;">"over 1 billion"</span>
                    </div>
                    <div style="padding:3px 0; color:#dfe5f2;">
                        <span style="color:#e8636f; font-weight:700;">"Highlighted"</span>
                        " world names are subsector capitals."
                    </div>
                </div>
            </Show>

            // --- settings / layers panel ---
            <Show when=move || panel.get() == 2>
                <div style=PANEL_STYLE>
                    <div style="display:flex; justify-content:space-between; align-items:center;">
                        <span style="font-weight:700; letter-spacing:0.05em;">"SETTINGS"</span>
                        <span on:click=move |_| panel.set(0)
                              style="cursor:pointer; color:#8a93a8; font-size:18px;">"✕"</span>
                    </div>
                    <hr style="border:none; border-top:1px solid #2a3145; margin:8px 0 4px;" />
                    <div style="font-weight:700; color:#aab3c8; margin:8px 0 2px;">"FEATURES"</div>
                    {toggle_row("Galactic Direction", opt_galactic)}
                    {toggle_row("Sector Grid", opt_grid)}
                    {toggle_row("Sector Names", opt_sector_names)}
                    {toggle_row("Borders", opt_borders)}
                    {toggle_row("Routes", opt_routes)}
                    {toggle_row("Region Names", opt_region_names)}
                    {toggle_row("Important Worlds", opt_important)}
                    <div style="font-weight:700; color:#aab3c8; margin:12px 0 2px;">"APPEARANCE"</div>
                    {toggle_row("More World Colors", opt_world_colors)}
                    {toggle_row("Filled Borders", opt_filled)}
                    {toggle_row("Dim Unofficial", opt_dim)}
                    <div style="font-weight:700; color:#aab3c8; margin:12px 0 2px;">"DEBUG"</div>
                    {toggle_row("Frame Timing (HUD)", opt_perf)}
                    <div style="margin-top:14px; padding-top:10px; border-top:1px solid #20283a; \
                                font-size:12px; color:#7e879c; line-height:1.5;">
                        "Not yet ported: style themes, milieu time-travel, \
                         and share links."
                    </div>
                </div>
            </Show>
        </main>
    }
}
