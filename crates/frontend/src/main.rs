//! Traveller Map client — Leptos, client-side rendering, compiled to WASM.
//!
//! Roams charted space: fetches the universe index + macro overlays, streams
//! the sectors overlapping the viewport, and renders client-side with LOD
//! styling. The canvas fills the window (device-pixel-ratio aware). All drawing
//! goes through `render` → `trait Canvas` (see `canvas.rs`).

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use tmap_core::dto::{Overlays, SearchResult, SearchResults, SectorData, Universe};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

mod canvas;
mod glyph;
mod render;
use render::ViewState;

const MILIEU: &str = "M1105";
/// Where the map opens — Spinward Marches grid coords.
const START: (i32, i32) = (-4, -1);
/// Safety cap: when zoomed out far enough to see more than this many sectors,
/// don't stream per-sector (Phase 5 macro overlays cover that range).
const MAX_STREAM: usize = 48;

/// Shared style for the top-right control buttons (home / key / hamburger).
const BTN_STYLE: &str = "width:40px; height:38px; border:none; border-radius:6px; \
    background:rgba(40,44,58,0.92); color:#e6ecf7; font-size:18px; line-height:1; \
    cursor:pointer; box-shadow:0 1px 4px rgba(0,0,0,0.5);";
/// Shared style for the floating panels (legend / settings).
const PANEL_STYLE: &str = "position:fixed; top:56px; right:12px; width:300px; \
    max-height:78vh; overflow:auto; box-sizing:border-box; padding:14px 18px 18px; \
    background:rgba(12,14,22,0.96); border:1px solid #2a3145; border-radius:10px; \
    color:#cfd6e6; font:14px system-ui,sans-serif; box-shadow:0 6px 24px rgba(0,0,0,0.6);";

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

fn win() -> web_sys::Window {
    web_sys::window().expect("no window")
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

/// Size the canvas drawing buffer to the window × devicePixelRatio (crisp on
/// retina). Returns the buffer size in device pixels.
fn size_canvas(canvas: &HtmlCanvasElement) -> (u32, u32) {
    let w = win();
    let dpr = w.device_pixel_ratio();
    let cw = w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0);
    let ch = w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(768.0);
    let bw = ((cw * dpr).round() as u32).max(1);
    let bh = ((ch * dpr).round() as u32).max(1);
    canvas.set_width(bw);
    canvas.set_height(bh);
    (bw, bh)
}

/// The canvas's CSS (logical) size — the coordinate space we draw in (the
/// context is DPR-scaled in `render::draw`).
fn logical_dims(canvas: &HtmlCanvasElement) -> (f64, f64) {
    (
        canvas.client_width().max(1) as f64,
        canvas.client_height().max(1) as f64,
    )
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

#[component]
fn App() -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let (status, set_status) = signal("Loading universe…".to_string());
    let (view, set_view) = signal(None::<ViewState>);
    let (results, set_results) = signal(Vec::<SearchResult>::new());
    let drag = RwSignal::new(None::<(f64, f64)>);
    // Canvas buffer size (device px); changes on mount and window resize.
    let (canvas_size, set_canvas_size) = signal((0u32, 0u32));

    // Off-reactive caches (read by reference in draw; `version` triggers redraws).
    let index = StoredValue::new(HashMap::<(i32, i32), String>::new()); // coord → name
    let sectors = StoredValue::new(HashMap::<(i32, i32), SectorData>::new());
    let inflight = StoredValue::new(HashSet::<(i32, i32)>::new());
    let failed = StoredValue::new(HashSet::<(i32, i32)>::new()); // sectors that errored — don't retry
    let overlays = StoredValue::new(None::<Overlays>);
    let (version, set_version) = signal(0u32);
    let (index_ready, set_index_ready) = signal(false);

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
    win()
        .add_event_listener_with_callback("resize", resize_cb.as_ref().unchecked_ref())
        .ok();
    resize_cb.forget(); // lives for the app's lifetime

    // 1) Load the universe index once.
    spawn_local(async move {
        match fetch_json::<Universe>(&format!("/api/universe?milieu={MILIEU}")).await {
            Ok(u) => {
                let map: HashMap<(i32, i32), String> = u
                    .sectors
                    .into_iter()
                    .map(|s| ((s.location.x, s.location.y), s.name))
                    .collect();
                set_status.set(format!("{MILIEU} — {} sectors · drag to pan, scroll to zoom", map.len()));
                index.set_value(map);
                set_index_ready.set(true);
                set_version.update(|v| *v += 1); // redraw so sector names show
            }
            Err(e) => set_status.set(format!("Universe load failed: {e}")),
        }
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
                let url = format!("/api/sector/{MILIEU}/{encoded}?lod=overview");
                match fetch_json::<SectorData>(&url).await {
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
                set_view.set(Some(render::fit_sector(lw, lh, START.0, START.1)));
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
                    render::draw(&canvas_el, &refs, ov.as_ref(), idx, v, opts);
                });
            });
        });
    });

    // --- input (mutates the view signal). Mouse coords are CSS px; scale by
    //     devicePixelRatio to match the device-pixel drawing buffer. ---

    let on_down = move |ev: web_sys::MouseEvent| {
        drag.set(Some((ev.client_x() as f64, ev.client_y() as f64)));
    };
    let on_move = move |ev: web_sys::MouseEvent| {
        let Some((lx, ly)) = drag.get_untracked() else {
            return;
        };
        let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
        drag.set(Some((x, y)));
        if let Some(v) = view.get_untracked() {
            set_view.set(Some(ViewState {
                center: (v.center.0 - (x - lx) / v.scale, v.center.1 - (y - ly) / v.scale),
                ..v
            }));
        }
    };
    let on_up = move |_: web_sys::MouseEvent| drag.set(None);

    // --- search ---
    let on_search = move |ev: web_sys::Event| {
        let q = event_target_value(&ev);
        if q.trim().is_empty() {
            set_results.set(Vec::new());
            return;
        }
        let encoded = String::from(js_sys::encode_uri_component(&q));
        spawn_local(async move {
            if let Ok(r) = fetch_json::<SearchResults>(&format!("/api/search?q={encoded}&milieu={MILIEU}")).await {
                set_results.set(r.results);
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

    // Home → the charted-space overview.
    let on_home = move |_: web_sys::MouseEvent| {
        if let Some(cv) = canvas_ref.get_untracked() {
            let (lw, lh) = logical_dims(&cv);
            set_view.set(Some(render::home_view(lw, lh)));
        }
        panel.set(0);
    };

    view! {
        <main style="margin:0; padding:0; overflow:hidden; background:#000;">
            <canvas node_ref=canvas_ref
                    style="position:fixed; top:0; left:0; width:100vw; height:100vh; \
                           display:block; cursor:grab; touch-action:none;"
                    on:mousedown=on_down
                    on:mousemove=on_move
                    on:mouseup=on_up
                    on:mouseleave=on_up
                    on:wheel=on_wheel></canvas>
            <div style="position:fixed; top:10px; left:12px; width:260px; \
                        font:14px system-ui,sans-serif; color:#cfd6e6;">
                <input type="search" placeholder="Search worlds & sectors…"
                       on:input=on_search
                       style="width:100%; box-sizing:border-box; padding:7px 10px; \
                              border-radius:6px; border:1px solid #2a3145; \
                              background:rgba(10,12,20,0.85); color:#e6ecf7; outline:none;" />
                <div style="margin-top:4px; max-height:60vh; overflow:auto; \
                            background:rgba(10,12,20,0.92); border-radius:6px;">
                    <For each=move || results.get()
                         key=|r| format!("{}/{}", r.sector, r.hex.clone().unwrap_or_default())
                         let:r>
                        <div on:click=move |_| {
                                 set_view.set(Some(ViewState {
                                     scale: 64.0,
                                     center: render::world_to_parsec(r.coord.x, r.coord.y),
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
            <div style="position:fixed; bottom:0; left:0; right:0; pointer-events:none; \
                        text-align:center; padding:5px 0; \
                        font:12px system-ui,sans-serif; text-shadow:0 1px 3px #000; \
                        background:linear-gradient(transparent, rgba(0,0,0,0.55));">
                <span style="color:#e32736; font-weight:700; letter-spacing:0.04em;">
                    "The Traveller Map"
                </span>
                <span style="color:#9aa3b8;">
                    " · Traveller © Mongoose Publishing (fair use) · \
                     data: the Traveller Map community (travellermap.com)"
                </span>
            </div>

            // --- top-right control cluster: home / key / hamburger ---
            <div style="position:fixed; top:10px; right:12px; display:flex; gap:6px;">
                <button on:click=on_home title="Home — charted-space overview"
                        style=BTN_STYLE>"⌂"</button>
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
            </div>

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
