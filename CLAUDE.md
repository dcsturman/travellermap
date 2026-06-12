# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Mission — this is a fork being rewritten

This repository is a **fork** of the upstream Traveller Map project (`inexorabletash/travellermap`). The C#/ASP.NET server and vanilla-JS client described below are the **reference implementation** — the existing, working system we are studying in order to **reimplement it**, not the code we are extending in place. The goals of the rewrite:

1. **Rust backend on standard Unix/Linux** (containerized as appropriate) — eliminate Windows, IIS, .NET Framework, and C#.
2. **Move rendering from the server to the browser.** Today the C# server renders map tiles/posters as images (server-side rendering). In the new design the **backend's sole job is to stream sector/metadata efficiently to the client**; all map rendering happens in the browser.
3. **Rust frontend using [Leptos](https://leptos.dev/), compiled to WASM**, for fast client-side rendering.

**Working model:**
- New Rust code lives in **parallel top-level directories** (e.g. a Rust backend crate and a Leptos frontend crate), *not* intermingled with the existing C#/JS tree. Leave the upstream files in place.
- **`res/` (sector data) stays common to both implementations.** It is shared with the parent project: we want to keep pulling data updates from upstream, and may contribute changes back. Treat `res/` as a shared, upstream-tracked asset — the Rust code consumes it, and edits there should remain compatible with upstream tooling/formats.
- The same applies to other potentially-shared assets (docs, schemas): prefer reuse over forking so contributing back stays feasible.

When working here, the default question is "**how does the reference implementation do this, and how should it be expressed in Rust/Leptos with client-side rendering?**" Use the C#/JS architecture below as the spec to port from.

**Roadmap:** the port proceeds in demoable phases tracked in **`PORT_PLAN.md`** (a living checklist — update it each session). Check there for current progress and the next step before starting work.

## Rust rewrite workspace (the new code)

A Cargo workspace at the repo root holds the rewrite. `res/` and the upstream C#/JS tree are left untouched alongside it.

```
Cargo.toml            # [workspace], resolver 2; default-members = core + backend
crates/
├── core/      (tmap-core)     # pure shared domain — no I/O, compiles native + wasm
│                              #   astrometrics (hex/coord math), dto (wire types)
├── backend/   (tmap-backend)  # axum + tokio; streams data, NO image rendering; :3000
└── frontend/  (tmap-frontend) # leptos CSR → wasm; renders in-browser; Trunk, :8080
```

- **`tmap-core` must stay I/O-free** (no tokio/fs/net) so its deps unify across native and wasm. Shared domain logic and the backend↔frontend wire contract (`dto`) live here. It is the landing zone for ported `Astrometrics.cs` / `SecondSurvey.cs` / `World.cs` logic.
- **`tmap-backend`** reimplements the data side of `server/api/` only — it streams `tmap_core::dto` types; it does **not** render images.
- **`tmap-frontend`** is where server-side rendering (`RenderContext.cs`, `Stylesheet.cs`) gets reimplemented to run in the browser.

### Datastore: none (decided)

**There is no database.** `res/` is the system of record (flat files, git-/upstream-tracked). The whole world dataset is small (thousands of sectors × ~500 worlds, all milieux) — it fits comfortably in RAM and is overwhelmingly read-only at runtime (data updates are an offline git pull, not user writes). The reference implementation's SQL Server was only a rebuildable search index over these same files, so it is being dropped entirely.

How the rewrite handles what SQL Server used to:
- **Bulk sector/world data:** loaded into an in-memory cache from `res/` (or precomputed per-sector blobs). Served directly — CDN-cacheable, stateless replicas scale horizontally for high read load.
- **Search:** an embedded Rust full-text index (**Tantivy**) built at startup from `res/`. This is a library/index, not a database or separate service — nothing to deploy or operate alongside the backend.

If a genuinely mutable, relational feature ever appears (accounts, saved campaigns, user annotations, user-uploaded sectors), reconsider then — but the core map needs no datastore. Don't introduce one speculatively.

### Zoom, level-of-detail, and the data-tile pyramid

This is the core decision for how the client-rendered map gets its data. **Stream a quantized data-tile pyramid keyed by `(tile, lod)`, not raw rectangles or the whole dataset.**

How the reference implementation handles zoom (the model we adapt): it is a **server-side raster tile pyramid** (a "slippy map"). `TileHandler` renders 256×256 PNG tiles on demand at a requested `scale` (pixels-per-parsec, clamped `2^-7 … 2^9`); the client (`map.js`) keeps a continuous `_logScale`, rounds it to an integer zoom level to choose tiles, fetches the tiles covering the viewport, and scales them by the fractional remainder for smooth zoom. Crucially, **level-of-detail is baked into rendering** via `Stylesheet` scale thresholds — e.g. `WorldMinScale = 4` (below ~4 px/parsec *no individual worlds are drawn*, only macro overlays + galaxy background), then `WorldBasicMinScale 24` / `WorldFullMinScale 48` / `WorldUwpMinScale 96` add per-world detail as you zoom in. So the payload per screen is ~constant at any zoom, and the client only ever downloads pixels.

We move rendering to the client, so the client needs **data**, not pixels — but the same pyramid + LOD structure applies, as a **vector/data tile pyramid** (Mapbox-vector-tile pattern):

- **Client asks for `(tile, lod)`; area↑ ⟹ LOD↓**, so payload stays bounded (`area × detail-per-feature` ≈ flat). The client picks the LOD from its zoom, requests the handful of tiles covering its viewport, and clips/assembles locally.
- **Quantize area into a fixed tile grid per LOD level** — do *not* serve arbitrary rectangles. Free-form rects = infinite cache-key space = no precompute, poor hit rate. A finite `(lod, x, y)` key space is what makes precompute + CDN caching work. Natural tiling units: multi-sector blocks at low LOD → one tile per sector → subsector/quadrant tiles at high LOD.
- **LOD collapses both axes together:** coarser LOD = fewer features *and* less detail per feature (positions-only dots; no UWP/stars/Ix/Ex/Cx). Map the tiers to the `Stylesheet` thresholds above.
- **The zoomed-all-the-way-out case is the *cheapest*, not the most expensive.** At the bottom LOD the pyramid degenerates to the single static macro-overlay set — the `res/Vectors/` borders/routes/rifts (~28 KB gzipped, sent once) over a galaxy background. No per-world data is needed at that zoom *by design* (that is why `res/Vectors/` + `res/labels/` exist as separate coarse datasets). So you never need "all worlds at once."
- **Precompute to static + CDN:** the data is static (changes only on an upstream pull), so the whole pyramid can be generated offline and served as static files — the backend degenerates to ~a static file server behind a CDN, matching the "no datastore, stateless" decision. Version the path (e.g. `/v/{dataVersion}/{lod}/{x}/{y}`); bump the version on a data pull instead of purging.

Data-size reality (why this is comfortable): all sectors across all milieux ≈ **24 MB raw**; one milieu's world data ≈ **1 MB gzipped**; a single sector (~443 worlds) ≈ **15 KB gzipped**; all macro overlays ≈ **28 KB gzipped**. A viewport at detail zoom spans 1–4 sectors → tens of KB. (Pragmatic MVP shortcut if per-tile generation is fiddly early on: ship the current milieu's lightweight positions layer on connect — ~1 MB — for instant navigation, then stream heavy per-sector detail on zoom-in.)

### Rust commands

- **Native build/test** (core + backend; the frontend is excluded from `default-members`): `cargo build`, `cargo test`. The whole workspace including the wasm crate: `cargo build -p tmap-frontend --target wasm32-unknown-unknown` or `cargo check --workspace --exclude tmap-frontend` patterns as needed.
- **Run the backend:** `cargo run -p tmap-backend` → listens on `http://127.0.0.1:3000` (`/api/health`, `/api/sector/sample`).
- **Run the frontend:** `trunk serve` from `crates/frontend/` → `http://127.0.0.1:8080`, with `/api` proxied to the backend (`Trunk.toml`), so run the backend alongside it.
- **Check the frontend compiles to wasm:** `cargo check -p tmap-frontend --target wasm32-unknown-unknown`.

A bare `cargo build`/`cargo test` at the root deliberately skips the wasm-only frontend (`default-members`) — build it via Trunk or with an explicit `--target wasm32-unknown-unknown`.

## What the reference implementation is

The source behind https://travellermap.com — an online map of the *Traveller* RPG universe. It is an **ASP.NET (.NET Framework 4.6.1) C# server** that renders map tiles, posters, and data APIs, plus a **vanilla-JavaScript (ES modules, no framework) client**. The bulk of the repository, however, is **sector data** under `res/` describing the Traveller universe across multiple time periods ("milieux").

## Platform note (reference implementation)

This is a **Windows / Visual Studio 2022 / IIS** project (`Maps.sln`). The server cannot be built or run on this macOS machine — `Maps.csproj` targets .NET Framework and depends on `System.Drawing`/`System.Web` and a separately-built **PDFsharp 1.5** DLL (see `SETUP.md`). On this machine, expect to work on **sector data editing** and **client-side JavaScript** rather than building the server.

## Commands

- **Lint client JS:** `npm install` once, then `npx eslint <file>.js`. ESLint config is `eslint.config.js`; `no-unused-vars` is off and `sw.js` is treated as a service worker.
- **Type-check JS:** `jsconfig.json` enables `checkJs` with `strictNullChecks` — the client is plain JS annotated with JSDoc types, validated by the TS language service (no build step, no transpilation).
- **C# unit tests:** MSTest project `unittests/UnitTests/` (Json, Serialization, Util) — run from Visual Studio's Test Explorer.
- **Integration tests:** `test/APITest.html`, `test/ContentTest.html`, `test/ImageTest.html` — open in a browser against a running server; image/content tests diff against reference data in `test/refs/`.
- **Validate sector data:** `tools/lintsec.html` (open in browser) checks SEC/tab data for errors.

## Server architecture (`server/`)

Request flow: `Global.asax.cs` registers **regex-based routes** (`server/http/Routing.cs`, `RegexRoute`) that map URL patterns to `IHttpHandler` classes. Route defaults (e.g. `accept: application/json`, sector/quadrant/hex captures) are passed as `RouteValueDictionary` values and read by handlers.

- **API handlers** (`server/api/`): one class per endpoint — `TileHandler`, `PosterHandler`, `JumpMapHandler` (images); `SearchHandler`, `SECHandler`, `SectorMetaDataHandler`, `CoordinatesHandler`, `JumpWorldsHandler`, `UniverseHandler`, `RouteHandler` (data). `DataHandlerBase`/`ImageHandlerBase` are shared bases.
- **Admin handlers** (`server/admin/`): `/admin/flush` (clears memory cache), `/admin/reindex` (rebuilds the SQL search index — required before search works; see `SETUP.md`), plus dump/errors/overview/codes/routes diagnostics. Gated by an admin key in `web.config`.
- **Domain model:** `World.cs`, `WorldCollection.cs`, `Sector.cs`, `SectorMap.cs`, `Astrometrics.cs` (hex ↔ coordinate math), `SecondSurvey.cs` (UWP/code parsing), `StellarData.cs`, `Location.cs`.
- **Rendering:** `RenderContext.cs`, `RenderUtil.cs`, `Stylesheet.cs`, `SectorStylesheet.cs` produce tiles/posters via `System.Drawing` and PDFsharp. **This is the logic that moves into the browser (Leptos/WASM) in the rewrite** — the new backend will not render images, so these files are the primary porting target for client-side rendering.
- **Serialization** (`server/serialization/`): parsers/writers for the several sector data formats — `SectorParser`, `SectorWriter`, `MSECParser`/`MSECWriter`, `SectorMetadataParser`/`SectorMetadataSerializer`.
- **Search** (`server/search/`): `SearchEngine` backed by SQL Server — but note the DB is a **derived, rebuildable index**, not a source of truth. `PopulateDatabase` builds the `sectors`/`subsectors`/`worlds`/`labels` tables entirely from the `res/` files on `/admin/reindex`. All other data (tiles, sector data, metadata) is served from files loaded into an in-memory cache; SQL is touched *only* for name/UWP/etc. search. In the rewrite there is **no database at all** — the search index becomes an embedded Tantivy index built from `res/`. See the "Datastore: none" note below.

`web.config` is gitignored — copy `Web.config.sample` and edit it (admin key, IISExpress port, SQL connection strings). See `SETUP.md`.

## Client architecture

No framework, no bundler. ES modules loaded directly by the browser; **Handlebars** for HTML templating.

- `map.js` — the core reusable map widget and JS API (tile loading, pan/zoom, coordinate conversion).
- `index.js` / `index.html` / `index.css` — the main site app built on `map.js`.
- `world_util.js` — UWP/world data helpers shared by client tools.
- `make/` — end-user tools for posters, booklets, borders, routes, and pathfinding over custom data.
- `print/` — print-format world sheets and posters.
- `sw.js` — service worker (offline support); intentionally separate global scope in lint config.

## Sector data (`res/`) — read before editing

The universe is organized by **milieu** (a snapshot of a year), each a directory under `res/Sectors/`: `M1105` is the canonical default (Imperial year 1105); others include `M0`, `M990`, `M1120`, `M1201`, `M1248`, `M1900`, plus non-OTU settings (`DeepnightRevelation`, `Orion OB1`, `Distant Fringe`, etc.). `res/Sectors/milieu.tab` indexes each milieu's metadata file.

Each sector has multiple representations:
- **`.tab`** — T5 Second Survey, tab-delimited with a header row (`Hex Name UWP Bases Remarks Zone PBG Allegiance Stars {Ix} (Ex) [Cx] Nobility W ...`). This is the primary editable world-data format.
- **`.sec`** — legacy fixed-column / Second Survey text format.
- **`.xml`** — sector *metadata*: name, allegiances, borders, routes, labels, subsector names.

**Critical — generated vs. hand-edited files:** Many `.tab` files in `res/Sectors/` begin with:
```
# Generated file - DO NOT MODIFY
# Update source files in res/t5ss/data instead
```
For those, **edit the source in `res/t5ss/data/*.tab`** and regenerate via `res/t5ss/update_world_data.pl` — do not edit the generated copy. Sectors **not** part of the T5SS dataset (e.g. `Zhdant`) have **no such header and are edited directly** in their `res/Sectors/<milieu>/` `.tab` file. Always check the file header before deciding where to make a change.

Format references: `doc/fileformats.html` (every sector file format) and `doc/secondsurvey.html` (meaning of each UWP/code field). `res/sectors.xsd` validates metadata XML.

### Two separate border systems (important)

Borders/routes on the map come from **two unrelated sources** — don't conflate them:

1. **Micro / per-sector** — `Border`/`Route` elements inside each sector's metadata `.xml` (`res/Sectors/.../*.xml`). Hex-accurate, tied to allegiance data, drawn at normal zoom.
2. **Macro** — hand-authored coarse overlay geometry in **`res/Vectors/`**, drawn only at galaxy/overview zoom (where per-hex borders would be invisibly fine). `RenderContext.cs` loads three sets: `borderFiles` (polity blobs — Imperium, Aslan, Vargr, Zhodani, Solomani, Hive, Kkree, client states), `riftFiles` (Great/Lesser/Windhorn/Delphi/Zhdant rifts), `routeFiles` (J4/J5/Core routes). Also `Galaxy_Positive.xvf` (galaxy background) and `res/labels/Worlds.xml` (macro world labels).

Each `res/Vectors/` file is a `VectorObject`: `Name`, a `MapOptions` flag, an Origin+Scale transform, `MinScale`/`MaxScale` (visibility zoom range), and `PathDataPoints` in absolute map (parsec) coordinates. These are **static overlay layers, not derived from or compiled into world data** — they render on top via their own passes (`DrawMacroBorders`/`DrawMacroRoutes`/`DrawRifts`/`DrawMacroNames`). In the rewrite they belong in the **metadata/overlay stream** (polyline + transform + visibility range + toggle flag), streamed once and drawn as client-side overlay layers — separate from the per-sector world stream.

### `res/` resource map (what each subdir is and who reads it)

Surveyed against the reference code so the port doesn't miss inputs. **"Server-parsed data"** = real inputs the new Rust backend must read/stream; **"render asset"** = images the C# renderer composites, which become client-side textures under client-side rendering; **"client asset"** = UI chrome the Leptos frontend needs an equivalent of but isn't part of the data model.

| Subdir | Kind | Consumed by / how |
| --- | --- | --- |
| `Sectors/` + `milieu.tab` | **Server-parsed data** | System of record — world data (`.tab`/`.sec`) + metadata (`.xml`) per milieu. |
| `Vectors/` | **Server-parsed data** | Macro border/route/rift overlay geometry (`VectorObject` XML) + `Galaxy_Positive.xvf`. See "Two border systems" above. |
| `labels/` | **Server-parsed data** | `Worlds.xml` (macro world dots), `minor_labels.tab`, `mega_labels.tab` — drawn by `RenderContext` `DrawMicroLabels`/`DrawMegaLabels` at specific zooms. |
| `t5ss/allegiance_codes.tab`, `t5ss/sophont_codes.tab` | **Server-parsed data** | Code→name lookup tables loaded by `SecondSurvey.cs`. **The rest of `t5ss/` is offline data-pipeline tooling** (`update_*.pl`, `data/`), not a runtime input — see the generated-`.tab` note above. |
| `styles/otu.css` | **Server-parsed data** | Default sector stylesheet, loaded by `Sector.cs` → `SectorStylesheet`; drives render styling. |
| `search/*.json` | **Server-parsed data** | Canned/pre-baked search results (published routes/tours) served by `SearchHandler` by query name. |
| `mains.json` | **Server-parsed data** (client-fetched) | World→"main" (cluster) mapping; fetched directly by `index.js`. |
| `maps/world_details.json` | **Server-parsed data** (client-fetched) | Extra per-world detail; fetched by `world_util.js`. (`maps/make_json.pl` generates it — offline.) |
| `Candy/` | **Render asset** | Galaxy/nebula/rift backgrounds + `Hyd0`–`HydA` hydrographics world icons + `Belt`; composited by `RenderContext`. Becomes client textures. |
| `markers/` | **Render/client asset** | Stock ship images for the marker API; overlaid client-side (`map.js` `drawMarker`/`AddMarker` by URL). |
| `print/` | **Client asset** | SVG/image parts for the printable world data sheet (`ex`/`cx`/`uwp`/`route`/`gg`/`stars`). (README's "world/" entry is stale — the dir is `print/`.) |
| `app/` | **Client asset** | PWA manifest, icons, favicons. |
| `ui/` | **Client asset** | Map UI chrome — logos, legends, cursors, spinners, `icons/`, `share/`. |
| `atlas/` | **Client asset** | Cover/logo art for the atlas maker (`make/atlas.html`). |
| `credits.js` | **Client asset** | Credits data, loaded client-side. |

## Conventions

- C# files use `#nullable enable`; code-analysis rules in `Maps.ruleset`.
- Client JS is type-checked through JSDoc — annotate new functions with JSDoc rather than adding a build step.
- Sector-data commits are conventionally prefixed with the milieu, e.g. `M1105: <description>` (see git history).
