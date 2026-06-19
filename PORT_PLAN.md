# Traveller Map — Rust/Leptos Port Plan

Living roadmap for the rewrite (see `CLAUDE.md` "Mission" and the Rust-rewrite section).
**Everything here is a numbered phase.** Phases 0–10 are done; 11+ are the remaining work.
A parallel **Callisto track** holds the experimental, feature-gated extensions. Update this
file as we go: tick boxes, fold in decisions, keep entries short.

## How we work

- **Agile + demoable.** Each phase ends in something you can run and see in the browser:
  `cargo run -p tmap-backend` (`:3000`) + `trunk serve` in `crates/frontend/` (`:8080`).
- **Order:** data over the wire → first pixels → make it a map → roam → zoom-out → styling →
  metadata → search → optimize → parity polish → deploy.
- Per phase, record what shipped + how to test + any decision that changed the plan.

## Baked-in tech decisions

- **All render logic in Rust/WASM behind a `Canvas` trait; Canvas 2D backend now, wgpu later.**
  The trait mirrors the reference `AbstractGraphics`, so `RenderContext`/`Stylesheet` is ported
  once and a `WgpuCanvas` can drop in without rewriting scene logic. WASM can't paint directly —
  `web-sys` is the binding to Canvas 2D/WebGPU; the win is that *logic* (scene, culling, LOD,
  parsing) is compiled WASM while the browser's native/GPU rasterizer does the final draw.
- **Parsing lives in `tmap-core`** (I/O-free): `.tab`/`.sec`/`.xml` → `dto` types. Backend does
  I/O + serving; frontend consumes JSON. Same parser will feed the future tile-precompute tool.
- **LOD is in the API contract from day one** (`?lod=`), but the parser is always full-fidelity —
  a low-LOD payload is a *projection*, never a different parse. Full `(lod,x,y)` tiling is deferred
  indefinitely — the current scheme (sector-by-name = the high-LOD tile, zoom-out drops to macro
  overlays) performs well (backend ~21k req/s), so build it only if profiling ever justifies it.
- Datastore: **none** (`res/` is the system of record, loaded into RAM). See `CLAUDE.md`.

---

# Completed phases (0–10)

**Phase 0 — Scaffold.** Workspace (`tmap-core`/`tmap-backend`/`tmap-frontend`), placeholder
backend + frontend page.

**Phase 1 — Real data over the wire.** `.tab` parser in `tmap-core` (`parse::parse_tab` →
`dto::World`, full 17-column fidelity, header-driven). Backend serves
`GET /api/sector/{milieu}/{name}?lod=full` (path-traversal guarded). *Test:* `cargo test
-p tmap-core`; `curl '…/api/sector/M1105/Spinward%20Marches?lod=full'` → 439 worlds.

**Phase 2 — First pixels.** `Canvas` trait + `Canvas2d` (web-sys) impl; renderer written only
against the trait. Leptos fetches one sector, draws labeled world dots + flat-top hex grid,
tinted by travel zone.

**Phase 3 — Pan & zoom.** `ViewState { scale (px/parsec), center (parsec) }` drives a uniform
map↔screen transform; drag pans, wheel zooms about the cursor. `scale` is px/parsec = the same
unit as the reference `Stylesheet` LOD thresholds, so later phases plug straight in.

**Phase 4 — Roam: multi-sector streaming.** Backend builds a per-milieu sector index (scans
`res/Sectors/{milieu}/*.xml`); `GET /api/universe`. Frontend switched to absolute map coords,
streams the sectors overlapping the viewport (+1 prefetch ring), off-reactive caches + in-flight
set. *Gotcha:* `pkill -f target/debug/tmap-backend` between runs (stale child kept serving).

**Phase 5 — Zoom out: macro overlays.** `VectorObject`/`Overlays` DTOs + `parse_vector_object`;
backend serves `res/Vectors/*.xml` at `GET /api/overlays` (borders/routes/rifts). LOD gating:
macro overlays at `scale ≤ 8`, worlds+grid at `scale ≥ 4`; `MIN_SCALE` 0.05. *Bugfix:* decode
the base64 `<PathDataTypes>` GDI+ bytes to split multi-subpath vectors (was drawing wedges).

**Phase 6 — Looks like Traveller: styling & detail tiers.** OTU palette from `Stylesheet`
(Red `#E32736`, Amber `#FFCC00`, water=blue, dry=white, black bg). Macro: red borders, white
dashed routes, region labels, procedural star field. World detail tiers by zoom (dot → name →
UWP). Sector/subsector boundary grids + rotated −50° watermark names. **Faithful `DrawWorld`
layout** in parsec units (hex# top, starport, disc colored by trade class, zone arc, UWP, name).
**Glyph table ported** (`glyph.rs` — scout/naval/military/… symbols, allegiance precedence,
red highlights). *Key fix:* DPR — draw in **logical (CSS) px** so `view.scale` matches the
reference calibration (was 2× on retina, shrinking everything).

**Phase 7 — Sector metadata: micro borders & routes.** `Border`/`Route` DTOs + parsers.
**Exact hex-edge borders:** backend computes the interior hex set; client fills the union and
strokes only region↔non-region edges (real hex-following border, no 150-line curve state machine).
Allegiance→color precedence: border `Color` attr → sector `<Stylesheet>` → `otu.css` table → gray.
*Seam fixes:* clip regions to their own sector's hexes (off-sector marker hexes were double-filling);
group borders by 2-char allegiance prefix so multi-domain polities (the Imperium) are continuous.

**Phase 8 — Search (first pass).** In-memory name index per milieu (exact > prefix > contains),
`GET /api/search`. Frontend search box → live results → click jumps the view. *(Superseded by
the Tantivy + query-language work in Phase 12.)*

**Phase 9 — Optimize streaming.** Response cache per `(milieu,name,lod)`; ETag + `If-None-Match`
→ 304, `Cache-Control: no-cache`. LOD projection `?lod=overview` (−44% payload); client requests
`overview`, `full` reserved for the detail panel. Offline static-tile precompute deferred to deploy.

**Phase 10 — Coverage, parity & performance (foundational pass).**
- **Coverage 87 → 190 sectors:** index built from the milieu **region list** (`M1105.xml`,
  authoritative coords + per-sector `DataFile`/`Type`); added the **column parser** (`parse_column`)
  and **legacy `.sec` parser** (`parse_sec`, regex-driven — 37 `.sec` sectors were silently empty).
  Renamed-sector metadata resolved via `MetadataFile`. Non-UTF-8 (`read_text`), case-insensitive
  file resolution (`resolve_ci` — a Linux/CI-only bug), failed-sector memo. Regression test:
  every M1105 sector serializes (190, 0 failures).
- **Visual gaps closed:** allegiance code right of each world; border corner gaps / double-stroke;
  hex-fill anti-alias seams (inflate ~3%); intra-Imperium domain seams (2-char prefix grouping);
  `CONTENT_SCALE 1.3` so glyphs fill the hex; sector-name fade, dynamic data-source footer, galaxy
  background image, placeholder/anomaly glyphs (`*`/`⌖`), style-value parity (zone arc, gray micro
  routes, scale-faded gray grids, log interpolation, dotmap disc radius), far-zoom minor-region red
  labels. **Home/Key/Hamburger** control cluster + full **Map Legend** parity + **Settings** toggles
  (`RenderOptions` threaded through `draw`). Backend static route `GET /api/res/{*path}`. *(Note: the
  reference does **not** fill macro polity borders — `VectorObject.Draw` strokes only; the filled
  look is the micro layer at s≥4, which we match. Confirmed 2026-06-18; macro↔micro handoff already
  exact at scale 4.)*
- **Profiling:** release build + in-app **per-layer frame-timing HUD** (Settings → DEBUG).
  Borders were the hot layer → **cached per-sector world-coord `Path2d` + canvas transform**
  (rebuild ~8 ms → 1–2 ms; same for the hex grid and dot-tier worlds). Cold load test (vegeta):
  **~21k req/s, p99 ≈ 1.6 ms — backend is not the bottleneck**; no backend optimization warranted.
- **Important Worlds** (`Worlds.xml` → Wheat dot + red name). **Mobile/touch:** one-finger pan,
  two-finger pinch-zoom; iOS viewport/buffer sizing fixed (`dvh`, `visualViewport` resize).
- *Render module split (`9baad5ef`):* `render.rs` → `render/` (one file per pass over a shared
  `common.rs`), to give the parity/theme work clean per-file ownership.

## Phase 10+ — interactive reference features (done)

Reference features built alongside/after the Phase 10 parity pass.

- [x] **Jump-route planner (2026-06-12/14).** Backend A* (`tmap-core::route`, `BinaryHeap`,
  spatially bucketed) over a per-milieu coord→world index; `GET /api/route` (+ `nored`/`wild`/`im`/`aok`
  filters), mirroring `RouteHandler.cs`. Full planner panel: Start/Destination (name, `Sector hhhh`,
  or click-map), J-1…J-6/H-1, the 4 routeOptions, waypoint list + leg distances, Print/Copy,
  `draw_jump_route` on the map.
- [x] **World detail panel (click-a-world).** `tmap_core::world_util` (ported `world_util.js`
  decoders + tables, 16 tests); `world_panel.rs` detail sheet (thumbnail, UWP glyphs, {Ix}/(Ex)/[Cx],
  system, population, bases, nobility, remarks, zone) at overview LOD, upgraded in place from
  `?lod=full`; `world_print.rs` print sheet; per-J range view (jump-N neighborhood highlight).
  *S3 world images — DECIDED skip* (only ~14 bespoke globes exist); `res/Candy/` generics used.
- [x] **Milieu / time selector (`9bdd28bf`).** Clock button → panel of the 8 curated OTU eras;
  switching tears down all per-milieu state + caches and re-fetches (stale-response guarded).
- [x] **Share tab — MVP (2026-06-16).** Link + embed (`<iframe>`) panel; live `share_url`. Scheme:
  our own `?cx&cy&scale&milieu` (a single swap-point — `build_share_url`/`parse_share_params` — for
  the future travellermap.com `p=x!y!logScale` compat). Reads params on load; reflects the live view
  via debounced `history.replaceState`. *TODO (Phase 12):* travellermap URL compat, Save Snapshot/PDF.
- [x] **Help / About / Credits (2026-06-18).** `?` tab: controls quick-help + Apache-2.0-compliant
  attribution (derivative-work notice, © 2006–2023 Joshua Bell, license + upstream links) + Mongoose
  trademark/Fair-Use notice.
- [x] **Prominent search bar + jump-route toggle**, **Dim Unofficial Data**, **mobile polish**
  (top-bar restack, tap-outside-to-close panels, dynamic-viewport modals), **CI** (`2026-06-16`:
  native clippy `-D warnings` + tests; wasm `cargo check` for default + callisto).

---

# Remaining phases (11+)

## Phase 11 — Style themes 🔨 IN PROGRESS

The Poster / Atlas / Print / Draft / FASA / Terminal / Mongoose presets (+ Candy, deferred).
Plan + per-preset values cited from `Stylesheet.cs`: **`STYLE_THEMES_PLAN.md`**. A theme is a
small palette+flags struct; all geometry/LOD is shared and already ported.

- [x] **A — Theme plumbing + default extraction (2026-06-18).** `render/theme.rs`: a `Theme`
  struct + `Theme::poster()` holding today's exact colors, threaded as `&Theme` through every pass.
  Pixel-identical by construction (each literal replaced with a field whose value equals it).
- [x] **B–D — 7 presets + selector (2026-06-18).** Decision (user): keep Poster's current custom
  tints; the alternates are **transcribed verbatim from `Stylesheet.cs`** (the `switch (style)`
  block + the `DefaultTo` cascade — world text ← foreground, hex# ← light, highlights ← highlight,
  stars ← foreground). Each preset is `Self { <overrides>, ..poster() }`, mirroring the C# cases.
  Threaded the new behaviors: per-preset `font`, `background`, world water/dry/**outline**/plain
  (`showWorldDetailColors`), split `red_zone`/`highlight`, `micro_border`/`micro_route`/
  `micro_border_text` overrides (draw-time, no cache rebuild), grid override, `uppercase_worlds`,
  the `worldDetails &= ~…` field drops (FASA/Draft/Mongoose), `show_galaxy`/`show_rift`. Settings →
  **STYLE** selector (red-highlighted, like the milieu picker); switching `render::clear_caches()`
  then redraws (the world-dot cache bakes colors). **Not yet replicated (flagged in `theme.rs`):**
  curved micro borders (FASA), all-hex numbering + subsector hex coords (Draft/FASA/Terminal), the
  Mongoose glyph re-layout + zone-perimeters + filled-UWP, text scale-expansion, and macro-name
  fonts. **Candy deferred** (needs per-world globe images + nebula background — out of scope) but
  planned to fully support later (user).
- [x] **C tail — `&style=` URL round-trip (DONE 2026-06-18).** `build_share_url` appends
  `&style=<preset>` (omitted at the default Poster); `parse_share_params` reads it back
  (case-insensitive, validated against `Theme::PRESETS`) and seeds the `style` signal on load.
  The debounced address-bar reflection includes it, so a shared link / reload restores the style.
  Param name matches travellermap.com's (forward-compatible with the reference-URL work).
- [ ] **Candy** preset 🔨. **All assets are already local** (`res/Candy/`: `Galaxy.png`, `Nebula.png`,
  `Rifts.png`, `Hyd0`–`HydA` + `Belt`), served at `/api/res/Candy/`; the world-detail thumbnails already
  load the `Hyd*` set. (The *photographic* per-world globes are a separate, optional detail-popup feature
  from the public S3 bucket `travellermap.s3.amazonaws.com/images/worlds/{Abbr} {Hex}.png` — public, CORS
  `*`, keyed by sector-abbrev+hex; not needed for the Candy map style.) All values below are verbatim from
  `Stylesheet.cs case Style.Candy:` (790–860) + defaults; cite-checked against a reference screenshot.
  - **Palette/flags (Phase 1 — no asset deps):**
    - Background **`#000000`** (Candy never sets it → Poster black); galaxy ON (inherited); rifts forced ON
      (`Stylesheet.cs:271`) → `show_rift:true`.
    - **Borders are per-polity, same as Poster** — `border.Color ?? allegianceStylesheet ?? microBorders.pen.color`
      (`RenderContext.cs:1825-1830`); metadata borders carry no `Color=`, so each polity colors from `otu.css`.
      Candy's `microBorders.pen.color=FromArgb(128,Red)` is only the no-color fallback → **`micro_border:None`**
      (do NOT override). No internal Imperium domain seams (our merged-prefix union already matches).
    - **Polity labels Amber `#FFCC00`** — `microBorders.textColor` default (`Stylesheet.cs:405`), Candy doesn't
      change it → `micro_border_text` stays Amber (NOT red).
    - **Sector/subsector watermark `rgba(218,165,32,0.502)`** = `FromArgb(128,Goldenrod)` (`:838`);
      `fadeSectorSubsectorNames=false` (`:796`) → `name_full/dark/dim` all that color, no fade tiers.
    - **Amber zone `#daa520` Goldenrod** (`:825`); red zone color unchanged.
    - **worldDetails drops:** Starport, Allegiance, Bases, Hex (`:817-818`) → `drop_starport/allegiance/bases`.
    - **Uppercase + non-uniform text `Scale` ("different font" — Arial family unchanged, `:217`)**: worlds
      `Scale(1.0,0.5) Tr(0,0)` (`:850-853`); sectorName `Scale(0.5,0.25) Tr(0,-0.25)` (`:828-831`); subsectorNames
      `Scale(0.3,0.15) Tr(0,-0.25)` (`:833-836`); microBorders `Scale(1.0,0.5) Tr(0,0.25)` (`:840-843`); all
      Uppercase. This vertical-squish transform is the defining Candy look — **in Phase 1 scope**, needs a
      per-label scale/translate transform in `worlds.rs` (names) + `labels.rs` (sector/subsector/border).
  - **Phase 1 — DONE 2026-06-19.** `candy()` preset + palette/flags; `uppercase_labels` wired into all label
    sites; **text transforms** — watermarks horizontal (`name_rotation=0`) + non-uniform `Scale`
    (sector `(0.5,0.25)`, subsector `(0.3,0.15)`), world names vertical-squished `(1.0,0.5)` via a generalized
    `fill_text_rotated(scale_x,scale_y)`. New `Theme` fields `name_rotation`/`sector_name_scale`/
    `subsector_name_scale`/`world_name_scale`. Auto-listed in the STYLE selector + `&style=candy`.
  - **Phase 2 — DONE 2026-06-19.** Nebula tiling (`stars::draw_nebula`, 2048px world-anchored tile, shown when
    `deepBackgroundOpacity<0.5`); world-globe compositing (`worlds::draw_world_images`) — `Hyd{0-A}`/`Belt` from
    `imageRadius(Size)`, decorations laid out **to the right** on a growing `decorationRadius` ring: 4-arc
    near-full **zone circle**, gas-giant disc, UWP, then the squished left-aligned **name** (matches
    `RenderContext.cs:1356-1481`). `mod.rs` swaps dots+glyphs → globes for Candy at detail zoom; nebula drawn
    below galaxy. New `Theme` fields `use_world_images`/`show_nebula`.
  - **Phase 3 (defer, "not replicated"):** curved micro borders (cardinal spline, tension 0.6 stroke/0.5 fill,
    `SVGGraphics.cs:677-736` + `RenderUtil.cs` `BorderPath` edge-walk; benefits FASA too — own track), the
    gas-giant Saturn **ring**, **hide the per-parsec hex grid in Candy** (`parsecGrid.visible=false` +
    `hexStyle=None`, `Stylesheet.cs:800,805` — needs a `show_hex_grid` theme flag gating `draw_hex_grid`),
    sector/subsector grid dash `{10,8}`/width, `Shadow` text background, per-scale border/route width taper,
    `hexContentScale`, and the scale-gated Candy name/UWP thresholds (`CandyMin*Scale`).

## Phase 12 — Public API compatibility

Our backend exposes a **private snake_case contract** (`/api/sector`, `/api/universe`, …) that
diverges from the documented public API (`/api/{verb}`, `/data/{sector}/…`, PascalCase, JSONP/XML).
Full matrix + decisions in **`PORT_API_COMPAT.md`** (the live tracker — don't duplicate it here).

- [x] **Search rebuilt on Tantivy (2026-06-17).** `/api/search` backed by an embedded Tantivy
  index per milieu (`add22936`), used as a **RegexQuery/TermQuery engine over raw fields** (not
  BM25) to preserve exact LIKE/SOUNDEX parity. Full query language ported (`tmap_core::searchlang`:
  `exact:`/`like:`/`uwp:`/`pbg:`/`zone:`/`alleg:`/`ex:`/`cx:`/`ix:`/`stellar:`/`remark:`/`in:`,
  `% _ []` wildcards, multi-word AND). **Ranking is ordered-parity** with live travellermap.com
  (`{Ix}` importance desc, tie-broken by each kind's SQL `DISTINCT` coordinate order — `f5a1936d`),
  enforced by 12 live-parity tests (gated by `TMAP_SKIP_PARITY` off CI). Canonical envelope
  `{"Results":{"Count","Items":[{"World"|"Sector"|"Subsector"|"Label":…}]}}`.
- [x] Search envelope made API-compatible (`2026-06-16`); JSONP + XML content negotiation across
  universe/search/credits/jumpworlds/route (`29ce0405`); Aslan-interior inline borders from the
  milieu region list (`60dd139d`). Metadata XML, `/data` aliases, POST `/api/sec`+`/api/metadata`.
- [x] **All documented data endpoints + shape gaps closed.** `/api/coordinates`, `/api/jumpworlds`,
  and the full `/data/{sector}/…` URL family are implemented + live-parity-tested; the search
  specials (`(random world)` + canned `(name)`→`res/search/*.json`) landed `2026-06-18`. **Decision
  taken:** adopt the public PascalCase shapes + documented URLs as the single contract (not a parallel
  private one) — see `PORT_API_COMPAT.md`. Only render endpoints (tile/poster/jumpmap) remain N/A by
  design (client-side rendering).

## Phase 13 — Polish & quality

- [ ] **Top control-bar order + Milieu into Settings (user, 2026-06-19).** Reorder the top-right
  control cluster to: **Home, Settings, Share, Key, Help**. Move the **Milieu** picker *out* of the top
  bar and *into* the Settings panel (same pattern as the STYLE selector already there).
- [ ] **Credits: always-prefix Mongoose ownership (user, 2026-06-19).** The line *"The Traveller game in
  all forms is owned by Mongoose Publishing. Copyright 1977 – 2024 Mongoose Publishing."* must prefix all
  other (sector/author) credits in the data-source footer. OK to wrap onto an extra line.
- [x] **World-detail panel tails — DONE 2026-06-19.** (a) **Generate World Map** link: ported the
  reference `world_util.js` `travellerworlds.com` generator URL (same params + seed) — shown **only in
  the non-`callisto` build**; the Callisto build keeps its in-app worldgen "World Map" button instead.
  (b) **Placeholder UWPs** (`XXXXXXX-X`/`???????-?`) now render an "Unsurveyed — no system data" note
  (+ allegiance/zone) instead of decoding gibberish (`is_placeholder_uwp`, reference `isPlaceholder`).
  (c) **Resource Units** surfaced in the Economics section — `tmap_core::world_util::resource_units`
  (RU column when present, else computed `R×L×I×E`, 0→1, signed efficiency; tested vs Regina = 6370).
- [x] **Frontend clippy gated — DONE 2026-06-18.** Cleaned the frontend wasm clippy (real fixes +
  named structs over two complex tuple types; `#[allow(too_many_arguments)]` only on the wide-but-
  flat render entry points). The wasm CI job now runs `cargo clippy … -- -D warnings` for both the
  default and `callisto` feature sets (replacing the bare `cargo check`).
- [x] **`cargo fmt --check` gated — DONE 2026-06-18.** Tree made rustfmt-clean; the native CI job
  runs `cargo fmt --all -- --check` (target-independent, covers the wasm frontend). CI is now fully
  deny-gated: native = fmt + clippy + tests; wasm = clippy `-D warnings` × (default, callisto).

## Phase 14 — Deploy (mostly done)

- [x] **Single-container service (2026-06-15).** One container serves API + WASM frontend from one
  origin (relative `/api`). Multi-stage `Dockerfile` (Trunk `--release --features callisto` → cargo
  → `debian-slim` + `dist/` + `res/`); backend binds `0.0.0.0:$PORT`, SPA fallback, universe warm-up.
- [x] **Cloud Run deploy scripts (2026-06-15).** `scripts/build.sh` (local verify) + `scripts/deploy.sh`
  (`gcloud builds submit` → Artifact Registry → `gcloud run deploy`, scale-to-zero). Custom domain
  `travellermap.callistoflight.com` (`DEPLOY.md`). Admin flush gated behind `TMAP_ENABLE_ADMIN`.
- [x] **CDN (Cloudflare) — code + scripts done (2026-06-18).** Data endpoints now send cacheable
  `public, max-age=300, s-maxage=86400, stale-while-revalidate` (+ETag) instead of `no-cache`, so the
  hot sectors edge-cache; `scripts/purge-cdn.sh` flushes the edge on every deploy (wired into
  `deploy.sh`, optional `CF_ZONE_ID`/`CF_API_TOKEN`). Remaining is the one-time Cloudflare dashboard
  setup (proxy the record, Full-strict SSL, a `/api`+`/data` cache rule) — documented in `DEPLOY.md`.

*Test:* `scripts/build.sh run` → full app on `:8080`; `scripts/deploy.sh` → live on Cloud Run.

---

# Callisto track (non-reference, feature-gated)

Experimental extensions **beyond** travellermap.com, gated behind the Cargo feature `callisto`
(**OFF by default, never committed enabled** — default builds, CI, and shipped artifacts stay
clean). The map images come from the **external worldgen service** (`tools.callistoflight.com`);
travellermap has no worldgen dependency. Spec: `worldgen/docs/library-integration.md`.

- [x] **Double-click a system → solar-system view (2026-06-13).** Builds the world's T5 fields into
  a worldgen request and shows the result in a zoom/pan popup (`Loading`/`Ready`/`Error` state
  machine, spinner + elapsed counter, Reset/Print/Download/Close). *(Re-architected from an optional
  native worldgen dep to the external HTTP service — even an optional dep must be Cargo-resolved,
  breaking standalone/CI builds.)*
- [x] **"World Map" button → main-world surface map (2026-06-15).** `/api/world` PNG (deterministic
  seed + GCS cache). Now **orbit-consistent** with the in-system double-click: probes `/api/system_svg`
  for the main world's *generated* orbit (rolled, not always 3) so both paths hit the same cache entry.
- [x] **Interactive SVG system map (2026-06-17).** Switched to `/api/system_svg` (bodies are
  `<g class="sysmap-body" data-*>`), inlined into the DOM: **hover/tap** any body → its UWP (or star
  spectral type); **double-click/long-press** a world/moon → its surface map. Wheel/drag + pinch
  zoom/pan in the popup. Reads `data-spectral` + nested companion-subsystem groups forward-compatibly.
  *(Companion-subsystem hover + star `data-spectral` need a worldgen redeploy to appear live — spec
  handed off to the worldgen repo; not edited from here.)*
