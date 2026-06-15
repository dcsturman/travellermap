# API Compatibility Audit — Rust Backend vs. travellermap.com `/doc/api`

> Generated 2026-06-12 (background agent). Source of truth: `doc/api.html` (local copy of
> https://travellermap.com/doc/api), cross-checked against `server/api/*.cs`. Our backend:
> `crates/backend/src/main.rs`, `crates/backend/src/search.rs`; wire types in `crates/core/src/dto.rs`.

## Headline finding — our wire contract diverges completely from the public API

Our backend exposes a **private contract** purpose-built for the Leptos client:
`/api/sector/{milieu}/{name}`, `/api/universe`, `/api/overlays`, `/api/search`, with **snake_case** JSON
custom shapes. The documented public API uses `/api/{verb}?params` + semantic `/data/{sector}/...` URLs and
**PascalCase** envelopes (`{"Results":{"Count":N,"Items":[...]}}`, `{"Sectors":[{"X","Y","Names":[...]}]}`,
`sx/sy/hx/hy`). So for **every** data endpoint, drop-in compatibility for existing clients/tools is currently
**Missing or Partial** — the underlying data is computed, but the URL shapes and JSON envelopes do not match.

**Decision needed (see PORT_PLAN cross-cutting):** do we want drop-in compatibility with existing
Traveller Map clients/tools (re-shape to PascalCase + documented URLs + `/data/...` aliases), or keep our
private contract for our own client and add a thin **compatibility layer** of documented endpoints alongside it?
The image endpoints (Tile/Poster/JumpMap) are intentionally N/A — those render client-side in this rewrite.

## Compatibility matrix

| Endpoint (URL pattern) | Purpose | Reference handler | Data / Render | Our status | Gap / what's needed |
|---|---|---|---|---|---|
| `GET /api/universe` • `GET /data` | List all sectors (X,Y, names, abbr, tags) for a milieu | `UniverseHandler.cs` | Data | **DONE (shape)** | **Unified:** `get_universe` now emits the public `{"Sectors":[{"X","Y","Milieu","Abbreviation","Tags","Names":[{Text,Lang?}]}]}` and **our Leptos client consumes it** (no more private snake_case shape). Per-sector output byte-identical to live (incl. `Tags` = sector tags + milieu metafile tag, e.g. `"Official OTU"`). Remaining: sector-set **completeness** (we return ~190 data-bearing M1105 sectors vs the reference's ~1021 positioned-but-dataless included), `era`/`requireData`/`tag` filters, `/data` alias, XML. |
| `GET /api/milieux` | List milieux (`Code`,`IsDefault`) | `UniverseHandler.cs` (`MilieuxCodesHandler`) | Data | **DONE** | `compat::get_milieux` — canonical milieu codes (`M<n>`/`IW`) in `milieu.tab` order, byte-for-byte vs live `[{"Code","IsDefault"}]`. +JSONP. |
| `GET /api/search?q=` | Name/attribute search → worlds/subsectors/sectors/labels | `SearchHandler.cs` | Data | **Partial** | Have `get_search` but URL/response mismatch (ref `{"Results":{"Count","Items":[{"World":{"SectorX","SectorY","HexX","HexY","Name","Sector","Uwp"}},{"Sector":{…}},{"Subsector":{…}},{"Label":{…}}]}}`); no query language (`* % ? _ []`, `exact:`/`like:`/`uwp:`/`pbg:`/`zone:`/`alleg:`/`stellar:`/`remark:`/`in:`, multi-word AND, `UWP` shortcut); no subsector/label hits; no XML/JSONP. Ours is plain substring over world+sector names. |
| `GET /api/coordinates?sector=&hex=` (+`subsector`,`sx/sy`,`hx/hy`,`x/y`) • `/data/{sector}/coordinates` • `/data/{sector}/{hex}/coordinates` | Sector lookup + coord conversion → `sx,sy,hx,hy,x,y` | `CoordinatesHandler.cs` | Data | **DONE** (`/data/...` aliases pending) | `compat::get_coordinates` — all param combos (sector by name/abbrev + hex/subsector, `sx/sy[+hx/hy]`, `x/y`) via `tmap_core::astrometrics::{location_to_coordinates,coordinates_to_location}`. Byte-for-byte vs live, +JSONP, 400/404. Not yet: `/data/{sector}/coordinates` URL aliases, XML. |
| `GET /api/jumpworlds?…` • `/data/{sector}/{hex}` • `…/jump/{jump}` | Worlds within N parsecs (0–12, default 6) | `JumpWorldsHandler.cs` | Data | **Missing** | Needs cross-sector neighborhood gather + hex-distance filter → `{"Worlds":[{Name,Hex,UWP,Bases,Remarks,Zone,PBG,Allegiance,Stellar,Ix,Ex,Cx,Nobility,Worlds}]}`. Per-world fields exist in `dto::World`; the multi-sector selector is new. |
| `GET /api/sec?sector=` (+`subsector`/`quadrant`/`sx/sy`) • `POST /api/sec` • `/data/{sector}` `/sec` `/tab` … | Sector UWP data as SEC / SecondSurvey / TabDelimited text | `SECHandler.cs` | Data | **DONE** (TabDelimited + SecondSurvey) | `get_sec` serves `type=TabDelimited` and `type=SecondSurvey` text (ported `SectorWriter` → `tmap_core::sector_writer`: TabDelimited + SecondSurvey + `ColumnSerializer`), with `subsector`/`quadrant`/`sx,sy` selection, `metadata`/`header`/`sscoords` toggles, the SEC/SecondSurvey `# …` metadata block, and synthesized abbreviations. Verified byte-for-byte vs live across many sectors. Remaining: **legacy fixed-column `SEC`** (the no-`type` default — needs T5→legacy allegiance/base encoders), `/data/{sector}[/sec|/tab]` URL aliases, `POST` reformat/lint. |
| `GET /api/metadata?sector=` (+`sx/sy`) • `POST` • `/data/{sector}/metadata` | Sector metadata (allegiances/borders/routes/labels/subsectors) XML/JSON | `SectorMetaDataHandler.cs` | Data | **DONE** (JSON) | `get_metadata` emits the full documented JSON (`tmap_core::metadata`: Selected/Tags/Abbreviation/Names/Credits/X/Y/Products/DataFile/Subsectors/Allegiances/Stylesheet/Labels/Borders/Regions/Routes). Parses the sector `.xml` **once** (vs the reference's per-field re-parse), computed border `LabelPosition`, worlds+borders-derived `Allegiances` with `Base`. Verified exact vs live for Spinward Marches (routes compared by resolved absolute coords). Remaining: XML output, `POST` convert/lint, `/data/{sector}/metadata` alias. Known minor diff: named colors stay verbatim (`olivedrab`) vs the reference's canonical casing (`OliveDrab`) — case-insensitive. |
| `GET /api/msec?sector=` • `/data/{sector}/msec` | MSEC (sec2pdf) metadata text | `MSECHandler.cs` | Data | **Missing** | No MSEC writer ported. Low priority. |
| `GET /api/credits?sector=&hex=` (+`sx/sy/hx/hy`,`x/y`) • `/data/{sector}/credits` • `…/{hex}/credits` | Per-location world data + sector credits/author/source | `CreditsHandler.cs` | Data | **Missing** | Needs sector metadata credit fields (Author/Source/Publisher/Copyright) + world lookup. JSON/XML/JSONP. (Pairs with the footer data-source credit in Phase 10.) |
| `GET /api/route?start=&end=&jump=` (+`wild`,`im`,`nored`,`aok`,`x/y`,`milieu`) | Shortest jump route between two worlds | `RouteHandler.cs` | Data | **Missing** | Needs a constrained pathfinder over the world graph (refuel/Imperial/red-zone). Significant logic — same engine as the Phase-10 jump-route planner. |
| `GET /t5ss/allegiances` | Allegiance code→name | `CodesHandler.cs` | Data | **DONE** | `compat::get_allegiances` — `res/t5ss/allegiance_codes.tab` → `[{"Code","LegacyCode","Name","Location"}]`, sorted by code. +JSONP. |
| `GET /t5ss/sophonts` | Sophont code→name | `CodesHandler.cs` | Data | **DONE** | `compat::get_sophonts` — `res/t5ss/sophont_codes.tab` → `[{"Code","Name","Location"}]`, sorted by code. +JSONP. |
| `GET /api/tile?x=&y=&scale=` | Render rectangle → PNG/JPEG/PDF/SVG | `TileHandler.cs` | **Render** | **N/A (client-render)** | Intentionally not served; client renders from data streams. |
| `GET /api/poster?…` • `POST` • `/data/{sector}[/{sub}]/image` | Render poster → image/PDF/SVG | `PosterHandler.cs` | **Render** | **N/A (client-render)** | Not served; `POST` custom-data path → client render of uploaded data. |
| `GET /api/jumpmap?sector=&hex=&jump=` • `POST` • `…/jump/{jump}/image` | Render jump map → image | `JumpMapHandler.cs` | **Render** | **N/A (client-render)** | Not served; its **data** sibling `/api/jumpworlds` IS a gap. |
| Main page `?scale=&options=&style=&sector=&hex=&q=&marker_*=&yah_*=&ew=…` | HTML app + query params/markers/Empress Wave | (app) | HTML/UI | **N/A** | Frontend (Leptos) concern; URL-param compat is a client matter. |
| `/go/{sector}[/{hex}]` • IFRAME embed (`forceui=1`, postMessage) | Short nav links / embed | (app) | HTML/UI | **N/A** | Frontend routing/embed concern. |

## Implementation status (2026-06-14)

**Done — `crates/backend/src/compat.rs`** (the "cheap" reshapes; pure additions, no collision with the private contract): `/api/coordinates`, `/api/milieux`, `/t5ss/allegiances`, `/t5ss/sophonts`. Each verified byte-for-byte against the live API (modulo the live server's non-functional `\/` slash-escaping), supports `&jsonp=<cb>`, and is guarded by unit tests (`compat::tests`). New pure math in `tmap_core::astrometrics`: `location_to_coordinates` / `coordinates_to_location`. **Common deferrals across all four:** XML output (`Accept: text/xml`) and the semantic `/data/{sector}/...` URL aliases — see the cross-cutting section.

**Done — `/api/universe` unified to the public shape.** Decision taken: **adopt the public PascalCase shape as the single contract** rather than keep a parallel private one. `get_universe` now emits `{"Sectors":[{X,Y,Milieu,Abbreviation,Tags,Names:[{Text,Lang?}]}]}` and our Leptos client deserializes that same shape (`UniverseResult`/`UniverseSector`/`SectorName` in `tmap_core::dto`; the private snake_case `Universe` is now in-memory only). Per-sector output is byte-identical to live. The index parse now retains all `<Name>`s (with `Lang`) and the `<Sector Tags>` attribute. **Why this works and isn't a parallel-path hack:** the public shape is a superset of what the client needs, so one URL serves both audiences. Our genuinely-private streaming endpoints (`/api/sector/{…}?lod=`, `/api/overlays`) have **no** public equivalent (the public API serves rendered tiles + per-format text; our browser renderer needs vector data) — they coexist as additive extensions and don't collide with any documented URL.

**Next — the "others":** `/api/search` envelope-unify is best folded into the planned Tantivy query-language work (one pass over search internals). The rest need new index/writer work: `/api/sec` text via `SectorWriter`; `/api/metadata` + `/api/credits` via a fuller metadata parse; `/api/jumpworlds` + `/api/route` via a cross-sector spatial index (`route` partly exists); `/api/msec` via `MSECWriter`. Plus the universe **completeness** gap (positioned-but-dataless sectors + `requireData`/`tag`/`era` filters).

## Compatibility test suite (TDD) — `crates/backend/src/compat_suite.rs`

An end-to-end suite drives the **real axum router in-process** (`tower::oneshot`) and checks every documented data endpoint against **golden fixtures captured from the live travellermap.com API** (`crates/backend/tests/refs/*`). JSON is compared as `serde_json::Value` (order- and `\/`-escaping-insensitive); the T5SS code tables are compared as a **set keyed by `Code`** (their array order is incidental — the reference uses .NET culture-sensitive collation). This is the parity oracle for the whole effort.

**TDD convention:** endpoints not yet implemented (or not yet in the public shape) have their test `#[ignore = "…"]`d **with the target assertion already written**, so implementing one = deleting its `ignore`. Scoreboard:

```
cargo test -p tmap-backend compat_suite                       # active (must pass)
cargo test -p tmap-backend compat_suite -- --include-ignored  # full target (red until done)
cargo test -p tmap-backend compat_suite -- --ignored --list   # what's left
```

Current: **15 active (green)** — coordinates (×6 forms + errors), milieux, t5ss allegiances/sophonts, universe envelope + known-sector shape, JSONP (wrap + bad-callback reject), sec TabDelimited + SecondSurvey, **metadata JSON**. **10 ignored (red targets)** — universe completeness (×2), search envelope, msec, credits, jumpworlds, route public bare-array shape, `/data/…` aliases (×2), `Accept: text/xml`.

Real gaps/bugs surfaced and fixed while building these tests: (1) `/t5ss/allegiances` was missing the 16 stock allegiances the reference merges on top of the `.tab` (M1120 splinter states + cultural regions; no `Location`); (2) `sector_credits` double-counted text (iterating every descendant and calling `.text()`), duplicating the `# Credits:` line; (3) sectors with no declared abbreviation now synthesize one (`Sector.SynthesizeAbbreviation`, e.g. `Zhdant`→`Zhda`) in both `/api/universe` and `/api/sec`; (4) world serialization order must be `(subsector, col, row)` — the reference enumerates a `WorldCollection` column-major, not file order. (Note: a few worlds differ from the *live* server purely because its `res/` is ahead of this checkout — data drift, not a writer bug; our output is faithful to local `res/`.)

## Our backend extensions (no reference equivalent — keep)
- **`GET /api/overlays`** — bundles macro borders/routes/rifts (`res/Vectors/`) + macro world labels (`res/labels/Worlds.xml`) into one JSON payload. New, well-motivated for client-side rendering; not part of the public API.
- **`GET /api/sector/{milieu}/{name}?lod=full|overview`** — combined worlds+borders+routes+subsectors JSON with LOD projection. Core streaming primitive of the rewrite; closest ref split is `/api/sec` (worlds) + `/api/metadata` (metadata).
- **`GET /api/res/{*path}`** — static passthrough of `res/` assets. Reference serves these as plain static files; documented marker URLs are root-relative `res/markers/...`, so the frontend (or a static mount) must also serve them at `/res/...` for documented marker URLs to resolve.
- **`POST /api/admin/flush`** — dev cache-flush; loosely mirrors ref `/admin/flush` (admin-key gated there).

## Prioritized data-API gaps (for drop-in compatibility)
1. **Coordinates API** — highest value-to-effort; math already in `tmap_core::astrometrics`. Parse `sector[&hex|&subsector]`/`sx/sy[&hx/hy]`/`x/y` → `{"sx","sy","hx","hy","x","y"}` (+XML/JSONP). `CoordinatesHandler.cs`.
2. **Search envelope + query language** — keep the route, emit the documented `Results.Items[].{World|Sector|Subsector|Label}` shape; add wildcard/prefix query language + subsector/label hits. Natural home for the planned Tantivy upgrade. `SearchHandler.cs` + `server/search/`.
3. **Universe alignment** — reshape to `{"Sectors":[{"X","Y","Milieu","Abbreviation","Tags","Name":[…]}]}`, add `/data` alias + `requireData`/`tag`/`era` filters + XML/JSONP. `UniverseHandler.cs`.
4. **SEC text output + `/data/{sector}[/sec|/tab]`** — emit `type=SecondSurvey|TabDelimited|Legacy` text + subsector/quadrant slicing + toggles. Need the **writers** (`SectorWriter`). `SECHandler.cs`.
5. **JumpWorlds API** — cross-sector gather + hex-distance filter → `{"Worlds":[…]}` PascalCase. `JumpWorldsHandler.cs`.
6. **Metadata (standalone)** — full sector metadata JSON/XML. `SectorMetaDataHandler.cs`/`SectorMetadataSerializer`.
7. **Lower:** `/api/milieux`, `/t5ss/allegiances`, `/t5ss/sophonts` (trivial table dumps); **Credits** (needs metadata credit fields); **Route** (constrained pathfinder — most effort, shared with the jump-route planner); **MSEC** (legacy, lowest).

## Cross-cutting compatibility requirements (all data endpoints)
- **PascalCase JSON envelopes** matching the reference `Results.*` classes (our snake_case DTOs won't deserialize in existing clients).
- **Content negotiation:** honor `Accept: text/xml` / `accept=text/xml` (reference default is actually XML; JSON is the common opt-in).
- **JSONP:** support `&jsonp=callback` on Coordinates/Credits/JumpWorlds/Search/Universe/Metadata/SEC/MSEC.
- **Semantic `/data/...` URL family** as aliases for `/api/...` verbs (sector by name OR T5SS abbreviation, e.g. `spin`).
- **Sector identification flexibility:** every sector-addressed endpoint accepts name, T5SS abbreviation, or `sx/sy` — our backend currently takes only exact sector name in the path.

---

## Compat-layer efficiency evaluation (2026-06-12)

Deciding factor for **thin compatibility layer vs. ground-up**: how efficiently each documented data endpoint can be served as a wrapper over our *current* in-memory model (universe index, lazily-built per-`(milieu,name,lod)` `SectorData`, per-milieu name search index, parse-once macro overlays, raw `res/`, `tmap_core::astrometrics`). Render endpoints (Tile/Poster/JumpMap) excluded.

| Endpoint | Compat cost | Why (work forced on our current model) | Recommendation |
|---|---|---|---|
| **Coordinates** | **Cheap** | All param combos resolve via the cached universe index + pure `astrometrics` math. No scan, no new structure. Just reshape to `{sx,sy,hx,hy,x,y}` + XML/JSONP. | **Compat-layer.** |
| **Universe** | **Cheap** | Full per-milieu index already in memory; reshape envelope. Caveat: `Tags` + localized `Names` aren't retained in `SectorIndexEntry` — thread them through (small change). Filters are cheap post-filters. | **Compat-layer** (+ retain Tags/Names). |
| **Milieux** | **Cheap** | Enumerate `milieu.tab` (tiny, once). Pure reshape. | **Compat-layer.** |
| **T5SS allegiances/sophonts** | **Cheap** | Two small static tables, cached, reshape to `[{Code,Name}]`. | **Compat-layer.** |
| **SEC/tab/SecondSurvey text** | **Moderate** | Data already parsed to full-fidelity `World`; cost is **new code not new data** — a faithful `SectorWriter` (dynamic widths, header/metadata blocks, `sscoords`), subsector/quadrant filtering, format-sniffing `POST`. Tractable per-request. | **Compat-layer + port `SectorWriter`.** |
| **Metadata** | **Moderate** | We hold only a **subset** (subsectors/borders+region/routes/border-colors). Documented doc also carries allegiances, labels, credits, `<Settings>`, full `<Stylesheet>`. Needs a fuller metadata parse + XML/JSON serializer, not just reshaping `SectorData`. Per-sector, cacheable. | **Mostly ground-up parse/serialize** (or accept a reduced doc). |
| **Credits** | **Moderate** | World lookup cheap; but credit fields (Author/Source/Publisher/Copyright) come from metadata we **don't parse** today. Shares the metadata-credit work above. | **Compat-layer once metadata credits parsed.** |
| **Search query-language** | **Expensive-as-compat** | Our index is a flat lowercased **name** list scored by substring — no attribute fields, no subsectors/labels, no wildcard/phonetic, no field-scoping. Bolting on `exact:`/`uwp:`/`alleg:`/`in:`/wildcards means re-scanning all worlds per query = rebuilding the index in disguise. | **Ground-up: the planned Tantivy inverted index** (name + UWP/PBG/Zone/Allegiance/Stellar/Remarks + subsectors/labels). Envelope is cheap; the matcher isn't. |
| **JumpWorlds** | **Expensive-as-compat** | Worlds within N parsecs straddle sectors; no cross-sector spatial index, so each call loads+scans up to ~9 neighbor `SectorData` and hex-filters. Heavy/repetitive. | **Ground-up (light): a per-milieu spatial/world index** (absolute-hex → world) so neighborhood gather is a bounded lookup. |
| **Route** | **Expensive-as-compat** | Shortest constrained path over a world graph that doesn't exist; a shim builds adjacency across many sectors **per call**, plus refuel/Imperial/red-zone predicates. | **Ground-up: same spatial index as JumpWorlds + A*/Dijkstra** with constraint predicates (shares the jump-route planner engine). |
| **MSEC** | **Moderate** | Legacy sec2pdf text; needs a ported `MSECWriter`. Low value. | **Compat-layer + `MSECWriter` (defer, lowest priority).** |

**Verdict.** *Thin compat layer, do first (pure reshape of data we already hold):* **Coordinates, Universe, Milieux, T5SS code tables** — Coordinates is the top value-to-effort win. *Compat layer but gated on a writer:* **SEC/tab text** (`SectorWriter`), **MSEC** (`MSECWriter`). *Gated on a fuller metadata parse (one shared piece of new work):* **Metadata + Credits**. *Justify ground-up (a shim would re-scan/rebuild per call):* **Search** → Tantivy inverted index; **JumpWorlds + Route** → one shared cross-sector spatial/world index (+ pathfinder for Route). For the ground-up trio the JSON envelope is a cheap final wrap; the cost is entirely the index/graph we don't have today.
