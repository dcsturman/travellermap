//! World detail panel — click a world on the map → its T5 data sheet.
//!
//! Mirrors the reference `#wds-template` (index.html) + `.wds-*` styles, decoding
//! every field client-side via [`tmap_core::world_util::decode_world`]. Collapsed
//! header (thumbnail, name, location, wiki link, Jump Route, expand toggle) plus
//! an expandable body with the full decoded UWP / extensions / population / etc.

use leptos::prelude::*;
use tmap_core::dto::World;
use tmap_core::world_util::{decode_world, Decoded, DecodedWorld};

/// The world the user clicked, with the sector context the sheet needs. `world`
/// is upgraded in place from the overview-LOD copy to the full-LOD copy once the
/// on-demand `?lod=full` fetch lands (`full` flips true), so the panel re-renders
/// with stellar/Ix/Ex/Cx/nobility/worlds filled in.
#[derive(Clone, PartialEq)]
pub struct SelectedWorld {
    pub world: World,
    pub sector_name: String,
    pub sector_coord: (i32, i32),
    pub subsector: String,
    pub full: bool,
}

/// Subsector letter `A`–`P` for a hex (8×10 parsec cells, 4×4 grid, reading
/// order) — used when the sector metadata doesn't name the subsector.
pub fn subsector_letter(col: i32, row: i32) -> char {
    let band_col = ((col - 1) / 8).clamp(0, 3);
    let band_row = ((row - 1) / 10).clamp(0, 3);
    (b'A' + (band_row * 4 + band_col) as u8) as char
}

/// Wiki URL for a world page (`makeWikiURL(name + " (world)")` in world_util.js),
/// with the sector/hex query the reference appends.
fn wiki_world_url(name: &str, sector: &str, hex: &str) -> String {
    let enc = |s: &str| String::from(js_sys::encode_uri_component(s));
    let suffix = enc(&format!("{name} (world)").replace(' ', "_"));
    format!(
        "https://wiki.travellerrpg.com/{suffix}?sector={}&hex={}",
        enc(sector),
        enc(hex)
    )
}

/// Thumbnail URL: the `res/Candy/` hydrographics generic (the reference's own
/// fallback) — `Belt` for an asteroid belt, else `Hyd{n}`.
fn thumb_url(d: &DecodedWorld) -> String {
    let file = if d.uwp.size.code == "0" {
        "Belt".to_string()
    } else {
        format!("Hyd{}", d.uwp.hydrographics.code)
    };
    format!("/api/res/Candy/{file}.png")
}

/// One `code → blurb` line in the expanded sheet (`.wds-field` style): a small
/// monospace code chip followed by its decoded meaning.
fn field_row(label: &str, code: &str, blurb: &str) -> impl IntoView {
    let label = label.to_string();
    let code = code.to_string();
    let blurb = blurb.to_string();
    view! {
        <div style="display:flex; gap:8px; padding:2px 0; align-items:baseline;">
            <span style="flex:none; width:74px; color:#8a93a8; font-size:11px; \
                         text-transform:uppercase; letter-spacing:0.03em;">{label}</span>
            <span style="flex:none; min-width:20px; font:600 13px ui-monospace,monospace; \
                         color:#e9eef9;">{code}</span>
            <span style="flex:1; color:#cfd6e6;">{blurb}</span>
        </div>
    }
}

/// A section heading inside the expanded body.
fn section(title: &str) -> impl IntoView {
    let title = title.to_string();
    view! {
        <div style="margin:9px 0 3px; color:#aab3c8; font-weight:700; font-size:11px; \
                    letter-spacing:0.06em; border-bottom:1px solid #20283a; padding-bottom:2px;">
            {title}
        </div>
    }
}

/// The list of decoded glyph rows for the full UWP.
fn uwp_rows(d: &DecodedWorld) -> impl IntoView {
    let u = &d.uwp;
    let rows = [
        ("Starport", &u.starport),
        ("Size", &u.size),
        ("Atmosphere", &u.atmosphere),
        ("Hydro", &u.hydrographics),
        ("Population", &u.population),
        ("Government", &u.government),
        ("Law Level", &u.law),
        ("Tech Level", &u.tech),
    ];
    rows.into_iter()
        .map(|(label, dec): (&str, &Decoded)| field_row(label, &dec.code, &dec.blurb))
        .collect_view()
}

/// Render the expanded body for a fully/partially decoded world.
fn expanded_body(sel: &SelectedWorld) -> impl IntoView {
    let d = decode_world(&sel.world);
    let w = &sel.world;

    // Allegiance line (full name, falling back to the bare code).
    let alleg = if w.allegiance.is_empty() {
        None
    } else {
        Some(
            d.allegiance_name
                .clone()
                .unwrap_or_else(|| w.allegiance.clone()),
        )
    };

    // System: stellar list + GG/belt counts + other worlds.
    let stars: Vec<_> = d
        .stars
        .iter()
        .map(|s| field_row("Star", &s.code, &s.blurb))
        .collect();
    let gg = d
        .pbg
        .gas_giants
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());
    let belts = d
        .pbg
        .belts
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());
    let system_line = format!("{gg} gas giant(s), {belts} planetoid belt(s)");
    let other_worlds = d.other_worlds.map(|n| format!("{n} other world(s)"));

    let bases = (!d.bases.is_empty()).then(|| d.bases.join(", "));
    let remarks: Vec<_> = d
        .remarks
        .iter()
        .filter(|r| !r.blurb.is_empty())
        .map(|r| field_row("", &r.code, &r.blurb))
        .collect();
    let nobility = (!d.nobility.is_empty()).then(|| {
        d.nobility
            .iter()
            .map(|n| n.blurb.clone())
            .collect::<Vec<_>>()
            .join(", ")
    });

    let ix = d.importance.clone();
    let ex = d.economics.clone();
    let cx = d.culture.clone();
    let pop = d.total_population.clone();
    let zone = d.zone.clone();
    let loading = !sel.full;

    view! {
        <div style="margin-top:8px; font-size:13px;">
            {alleg.map(|a| view! {
                {section("Allegiance")}
                <div style="color:#cfd6e6; padding:2px 0;">{a}</div>
            })}

            {section("UWP")}
            <div style="font:600 14px ui-monospace,monospace; color:#fff; padding:0 0 4px;">
                {w.uwp.clone()}
            </div>
            {uwp_rows(&d)}

            {ix.map(|ix| view! {
                {section("Importance {Ix}")}
                {field_row("Importance", &ix.imp, ix.blurb.as_deref().unwrap_or(""))}
            })}
            {ex.map(|ex| view! {
                {section("Economics (Ex)")}
                {field_row("Resources", &ex.resources.code, &ex.resources.blurb)}
                {field_row("Labor", &ex.labor.code, &ex.labor.blurb)}
                {field_row("Infrastructure", &ex.infrastructure.code, &ex.infrastructure.blurb)}
                {field_row("Efficiency", &ex.efficiency.code, &ex.efficiency.blurb)}
            })}
            {cx.map(|cx| view! {
                {section("Culture [Cx]")}
                {field_row("Heterogen.", &cx.heterogeneity.code, &cx.heterogeneity.blurb)}
                {field_row("Acceptance", &cx.acceptance.code, &cx.acceptance.blurb)}
                {field_row("Strangeness", &cx.strangeness.code, &cx.strangeness.blurb)}
                {field_row("Symbols", &cx.symbols.code, &cx.symbols.blurb)}
            })}

            {section("System")}
            {(!stars.is_empty()).then_some(stars)}
            <div style="color:#cfd6e6; padding:2px 0;">{system_line}</div>
            {other_worlds.map(|o| view! { <div style="color:#cfd6e6; padding:2px 0;">{o}</div> })}

            {section("Population")}
            <div style="color:#cfd6e6; padding:2px 0;">
                {pop.map(|p| format!("{p} inhabitants")).unwrap_or_else(|| "Unknown".into())}
            </div>

            {bases.map(|b| view! {
                {section("Bases")}
                <div style="color:#cfd6e6; padding:2px 0;">{b}</div>
            })}
            {nobility.map(|n| view! {
                {section("Nobility")}
                <div style="color:#cfd6e6; padding:2px 0;">{n}</div>
            })}
            {(!remarks.is_empty()).then(|| view! {
                {section("Remarks")}
                <div>{remarks}</div>
            })}

            {section("Travel Zone")}
            <div style="color:#cfd6e6; padding:2px 0;">
                {zone.rating.to_string()}" — "{zone.rule.to_string()}
            </div>

            {loading.then(|| view! {
                <div style="margin-top:8px; color:#7e879c; font-size:12px; font-style:italic;">
                    "Loading full system data…"
                </div>
            })}
        </div>
    }
}

/// The world detail panel. Shown when `selected` is `Some`. `on_close` clears the
/// selection; `on_plan_route` seeds the jump-route planner with this world;
/// `on_jump_range` toggles the jump-N neighborhood cutout (J-N opens a scoped
/// view of every world within N parsecs). `active_jump` reflects the current
/// rating (0 = none) so the J-N pills show their active state.
#[component]
pub fn WorldPanel(
    selected: RwSignal<Option<SelectedWorld>>,
    #[prop(into)] on_close: Callback<()>,
    #[prop(into)] on_plan_route: Callback<String>,
    #[prop(into)] on_print: Callback<()>,
    #[prop(into)] on_jump_range: Callback<i32>,
    /// Render the selected world's surface map (Callisto, dev-only). The button
    /// that fires this is only built under the `callisto` feature; in a default
    /// build the callback is unused.
    #[prop(into)]
    on_world_map: Callback<()>,
    #[prop(into)] active_jump: Signal<i32>,
) -> impl IntoView {
    let expanded = RwSignal::new(true);
    #[cfg(not(feature = "callisto"))]
    let _ = on_world_map;

    view! {
        <Show when=move || selected.get().is_some()>
            {move || {
                let Some(sel) = selected.get() else { return ().into_any() };
                let d = decode_world(&sel.world);
                let w = &sel.world;
                let title = if w.name.is_empty() { format!("{} {}", sel.sector_name, w.hex) } else { w.name.clone() };
                let location = format!("{} / {}", sel.subsector, sel.sector_name);
                let wiki = wiki_world_url(&title, &sel.sector_name, &w.hex);
                let thumb = thumb_url(&d);
                let plan_label = title.clone();

                // "World Map" action (Callisto, dev-only): a full-width button that
                // renders the main world's surface map. Built as an `AnyView` so the
                // panel embeds `{world_map_btn}` unconditionally; off by default.
                #[cfg(feature = "callisto")]
                let world_map_btn = view! {
                    <div style="flex:none; margin-top:8px;">
                        <button on:click=move |_| on_world_map.run(())
                                title="Generate this world's surface map"
                                style="width:100%; padding:7px 0; border-radius:15px; cursor:pointer; \
                                       border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                       color:#cdd5e6; font:600 12px system-ui;">
                            "🌍  World Map"
                        </button>
                    </div>
                }.into_any();
                #[cfg(not(feature = "callisto"))]
                let world_map_btn = ().into_any();

                view! {
                    <div style="position:fixed; top:56px; right:12px; width:300px; \
                                max-width:calc(100vw - 24px); max-height:calc(100dvh - 90px); \
                                box-sizing:border-box; display:flex; flex-direction:column; \
                                padding:12px 14px 14px; border-radius:8px; \
                                background:rgba(12,15,24,0.96); border:1px solid #2a3145; \
                                box-shadow:0 6px 26px rgba(0,0,0,0.6); \
                                font:13px system-ui,sans-serif; color:#cfd6e6; z-index:20;">
                        // --- header: thumbnail + name + location + close ---
                        <div style="flex:none; display:flex; gap:10px; align-items:flex-start;">
                            <img src=thumb width="44" height="44"
                                 style="flex:none; border-radius:50%; background:#000; \
                                        border:1px solid #2a3145;" />
                            <div style="flex:1; min-width:0;">
                                <div style="font:700 17px system-ui; color:#fff; line-height:1.1;">{title}</div>
                                <div style="color:#8a93a8; font-size:12px; margin-top:1px;">{location}</div>
                            </div>
                            <span on:click=move |_| on_close.run(())
                                  style="flex:none; cursor:pointer; color:#8a93a8; font-size:18px; \
                                         line-height:1; padding:0 2px;">"✕"</span>
                        </div>

                        // --- actions ---
                        <div style="flex:none; display:flex; gap:8px; margin-top:10px;">
                            <a href=wiki target="_blank" rel="noopener"
                               style="flex:1; text-align:center; padding:6px 0; border-radius:15px; \
                                      border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                      color:#cdd5e6; text-decoration:none; font:600 12px system-ui;">
                                "Wiki ↗"</a>
                            <button on:click=move |_| on_plan_route.run(plan_label.clone())
                                    style="flex:1; padding:6px 0; border-radius:15px; cursor:pointer; \
                                           border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                           color:#cdd5e6; font:600 12px system-ui;">
                                "Jump Route"</button>
                            <button on:click=move |_| on_print.run(()) title="Print data sheet"
                                    style="flex:none; width:34px; border-radius:15px; cursor:pointer; \
                                           border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                           color:#cdd5e6; font:600 13px system-ui;">
                                "🖨"</button>
                            <button on:click=move |_| expanded.update(|e| *e = !*e)
                                    title="Toggle details"
                                    style="flex:none; width:34px; border-radius:15px; cursor:pointer; \
                                           border:1px solid #2a3145; background:rgba(40,44,58,0.7); \
                                           color:#cdd5e6; font:600 13px system-ui;">
                                {move || if expanded.get() { "▾" } else { "▸" }}</button>
                        </div>

                        // --- world surface-map button (callisto, dev-only) ---
                        {world_map_btn}

                        // --- jump-range pills: J-N opens a scoped jump-N
                        //     neighborhood cutout (toggle the same N off). ---
                        <div style="flex:none; display:flex; gap:6px; margin-top:8px;">
                            {(1..=6).map(|n| view! {
                                <button title="View jump-N neighborhood"
                                        on:click=move |_| on_jump_range.run(n)
                                        style:background=move || if active_jump.get() == n { "#e32736" } else { "rgba(40,44,58,0.7)" }
                                        style:color=move || if active_jump.get() == n { "#fff" } else { "#cdd5e6" }
                                        style="flex:1; padding:0; line-height:26px; cursor:pointer; \
                                               border:1px solid #2a3145; border-radius:15px; \
                                               font:600 12px system-ui;">
                                    {format!("J-{n}")}
                                </button>
                            }).collect_view()}
                        </div>

                        // --- expandable body (scrolls) ---
                        <Show when=move || expanded.get()>
                            <div style="flex:1; min-height:0; overflow:auto; margin-top:4px;">
                                {expanded_body(&sel)}
                            </div>
                        </Show>
                    </div>
                }.into_any()
            }}
        </Show>
    }
}
