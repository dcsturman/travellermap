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
| `GET /api/universe` • `GET /data` | List all sectors (X,Y, names, abbr, tags) for a milieu | `UniverseHandler.cs` | Data | **Partial** | Data computed (`get_universe`), but URL is `/api/universe?milieu=`; missing `/data` alias + `era`/`requireData`/`tag` filters. Envelope differs: ref `{"Sectors":[{"X","Y","Milieu","Abbreviation","Tags","Name":[...]}]}` vs ours `{milieu, sectors:[{name,location:{x,y},abbreviation}]}`. Need PascalCase, `Accept: text/xml`, JSONP. |
| `GET /api/milieux` | List milieux (`Code`,`IsDefault`) | `UniverseHandler.cs` (`MilieuxCodesHandler`) | Data | **Missing** | Trivial: enumerate `res/Sectors/milieu.tab` → `{"Milieux":[{"Code","IsDefault"}]}`. |
| `GET /api/search?q=` | Name/attribute search → worlds/subsectors/sectors/labels | `SearchHandler.cs` | Data | **Partial** | Have `get_search` but URL/response mismatch (ref `{"Results":{"Count","Items":[{"World":{"SectorX","SectorY","HexX","HexY","Name","Sector","Uwp"}},{"Sector":{…}},{"Subsector":{…}},{"Label":{…}}]}}`); no query language (`* % ? _ []`, `exact:`/`like:`/`uwp:`/`pbg:`/`zone:`/`alleg:`/`stellar:`/`remark:`/`in:`, multi-word AND, `UWP` shortcut); no subsector/label hits; no XML/JSONP. Ours is plain substring over world+sector names. |
| `GET /api/coordinates?sector=&hex=` (+`subsector`,`sx/sy`,`hx/hy`,`x/y`) • `/data/{sector}/coordinates` • `/data/{sector}/{hex}/coordinates` | Sector lookup + coord conversion → `sx,sy,hx,hy,x,y` | `CoordinatesHandler.cs` | Data | **Missing** | All math in `tmap_core::astrometrics`; need handler for the param combos → `{"sx","sy","hx","hy","x","y"}` (+XML/JSONP). **High value** — many tools resolve name→coord here. |
| `GET /api/jumpworlds?…` • `/data/{sector}/{hex}` • `…/jump/{jump}` | Worlds within N parsecs (0–12, default 6) | `JumpWorldsHandler.cs` | Data | **Missing** | Needs cross-sector neighborhood gather + hex-distance filter → `{"Worlds":[{Name,Hex,UWP,Bases,Remarks,Zone,PBG,Allegiance,Stellar,Ix,Ex,Cx,Nobility,Worlds}]}`. Per-world fields exist in `dto::World`; the multi-sector selector is new. |
| `GET /api/sec?sector=` (+`subsector`/`quadrant`/`sx/sy`) • `POST /api/sec` • `/data/{sector}` `/sec` `/tab` … | Sector UWP data as SEC / SecondSurvey / TabDelimited text | `SECHandler.cs` | Data | **Partial** | We parse all three formats with full-fidelity `World`, but only emit our JSON. Missing: text output by `type=`, `/data/{sector}[/sec|/tab]` URLs, subsector/quadrant filtering, `metadata=0`/`header=0`/`sscoords=1`, `POST` reformat/lint. Need to port `SectorWriter`. |
| `GET /api/metadata?sector=` (+`sx/sy`) • `POST` • `/data/{sector}/metadata` | Sector metadata (allegiances/borders/routes/labels/subsectors) XML/JSON | `SectorMetaDataHandler.cs` | Data | **Partial** | We surface a *subset* inside `SectorData` (subsectors, borders→region, routes). No standalone endpoint, no full metadata JSON/XML matching ref schema, no `POST` convert/lint. |
| `GET /api/msec?sector=` • `/data/{sector}/msec` | MSEC (sec2pdf) metadata text | `MSECHandler.cs` | Data | **Missing** | No MSEC writer ported. Low priority. |
| `GET /api/credits?sector=&hex=` (+`sx/sy/hx/hy`,`x/y`) • `/data/{sector}/credits` • `…/{hex}/credits` | Per-location world data + sector credits/author/source | `CreditsHandler.cs` | Data | **Missing** | Needs sector metadata credit fields (Author/Source/Publisher/Copyright) + world lookup. JSON/XML/JSONP. (Pairs with the footer data-source credit in Phase 10.) |
| `GET /api/route?start=&end=&jump=` (+`wild`,`im`,`nored`,`aok`,`x/y`,`milieu`) | Shortest jump route between two worlds | `RouteHandler.cs` | Data | **Missing** | Needs a constrained pathfinder over the world graph (refuel/Imperial/red-zone). Significant logic — same engine as the Phase-10 jump-route planner. |
| `GET /t5ss/allegiances` | Allegiance code→name | `CodesHandler.cs` | Data | **Missing** | Trivial: serve `res/t5ss/allegiance_codes.tab` → `[{"Code","Name"}]`. |
| `GET /t5ss/sophonts` | Sophont code→name | `CodesHandler.cs` | Data | **Missing** | Trivial: serve `res/t5ss/sophont_codes.tab`. |
| `GET /api/tile?x=&y=&scale=` | Render rectangle → PNG/JPEG/PDF/SVG | `TileHandler.cs` | **Render** | **N/A (client-render)** | Intentionally not served; client renders from data streams. |
| `GET /api/poster?…` • `POST` • `/data/{sector}[/{sub}]/image` | Render poster → image/PDF/SVG | `PosterHandler.cs` | **Render** | **N/A (client-render)** | Not served; `POST` custom-data path → client render of uploaded data. |
| `GET /api/jumpmap?sector=&hex=&jump=` • `POST` • `…/jump/{jump}/image` | Render jump map → image | `JumpMapHandler.cs` | **Render** | **N/A (client-render)** | Not served; its **data** sibling `/api/jumpworlds` IS a gap. |
| Main page `?scale=&options=&style=&sector=&hex=&q=&marker_*=&yah_*=&ew=…` | HTML app + query params/markers/Empress Wave | (app) | HTML/UI | **N/A** | Frontend (Leptos) concern; URL-param compat is a client matter. |
| `/go/{sector}[/{hex}]` • IFRAME embed (`forceui=1`, postMessage) | Short nav links / embed | (app) | HTML/UI | **N/A** | Frontend routing/embed concern. |

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
