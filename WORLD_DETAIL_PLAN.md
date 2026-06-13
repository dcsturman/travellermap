# World Detail Panel — Implementation Plan

Plan for the click-a-world data-sheet panel (planning agent, 2026-06-13). Decode
everything client-side from the world's own T5 fields (no wiki fetch; wiki is an
outbound link). **Key finding: the `World` DTO already carries every field the
sheet needs** (`crates/core/src/dto.rs` — `uwp`, `pbg`, `bases`, `remarks`, `zone`,
`allegiance`, `stellar`, `importance`, `economic`, `cultural`, `nobility`,
`worlds`, `resource_units`) and the parser already populates them. **No DTO or
parser changes required.** The work: (1) port decoders, (2) add full-LOD
fetch-on-click, (3) build the panel UI, (4) wire hit-testing to a `selected_world`.

## 1. Click → select a world
The route planner already has the pieces in `crates/frontend/src/main.rs`:
- `fill_endpoint` does screen→parsec hit-testing via `v.to_parsec` +
  `render::sector_hex_parsec`, nearest loaded world within ~0.9 parsec. Refactor
  its core into a reusable `nearest_world(px) -> Option<(cell, hex, World)>`.
- `on_up` already distinguishes click vs drag via `down_pos`.
- Add `selected: RwSignal<Option<SelectedWorld>>` (cloned `World` + sector name +
  sector `Coord` + computed subsector letter). In `on_up`: when
  `is_click && !route_open`, call `select_world(up)`; when `route_open`, keep
  `fill_endpoint` (route mode wins — mutually exclusive, like the cursor toggle).
  Empty-space click or ✕ clears `selected`.

## 2. Data fetch / LOD
The streaming effect fetches visible sectors at `?lod=overview`, which strips
stellar/Ix/Ex/Cx/nobility/worlds/RU. The backend already serves `?lod=full`
(default) — **no backend change**. **Recommended: fetch the clicked sector at
`full` on demand** (don't bloat the bulk stream). Add a `full_sectors:
StoredValue<HashMap<(i32,i32), SectorData>>` cache; on select, read from it or
`spawn_local` a `fetch_json::<SectorData>("/api/sector/{MILIEU}/{name}?lod=full")`,
insert, look up by `hex`. Show the panel immediately with overview-level fields
(name/UWP/zone/pbg/bases/remarks survive `project_overview`) + a "Loading…"
expanded section filled when the full fetch lands.

## 3. Decoders to port → new pure module `crates/core/src/world_util.rs`
(`tmap-core`, not frontend: pure string→string, unit-testable, backend-reusable.)
Add `pub mod world_util;` to `crates/core/src/lib.rs`. Port from `world_util.js`:
`splitUWP` + STARPORT/SIZ/ATM/HYD/POP/GOV/LAW/TECH tables; `splitPBG`;
`splitRemarks` (regex via the `regex` crate already in core) + REMARKS_TABLE +
patterns + `decodeSophontPopulation`; `parseIx` + IX table; `(Ex)` split + 4 blurb
tables; `[Cx]` split + 4 blurb tables; `NOBILITY_TABLE`; `BASE_TABLE` (+ Zhodani
KM/W special case); `STELLAR_TABLE`/overrides/color + star-splitting (`M8 III` →
"Red Giant"); zone decode (rule + TAS rating + class); Worlds/OtherWorlds
arithmetic; `TotalPopulation` + thousands-commas; `SOPHONT_TABLE` (legacy static
first; extend from `res/t5ss/sophont_codes.tab` later). **Allegiance full name:**
bake `res/t5ss/allegiance_codes.tab` via `include_str!` into `world_util.rs` →
`allegiance_name(code)` (pure, no new endpoint; ~14KB).
**Stays frontend:** `makeWikiURL` (build link, don't fetch); the
`travellerworlds.com` map-generator link. **Skip for v1:** S3 world images +
client-side star-color compositing (`renderWorldImage`).

## 4. Panel UI → new module `crates/frontend/src/world_panel.rs`
`WorldPanel` component behind `<Show when=move || selected.get().is_some()>`.
Mirror the reference `#wds-template` (index.html) + `.wds-*`/`.ds-frame` CSS with
the inline-style approach used in `main.rs`.
- **Collapsed:** thumbnail; name; "{subsector} / {sector}" (needs a
  `subsector_letter(hex) -> char` helper + lookup in `info.subsectors`); wiki
  link (`target=_blank`); Jump Route button (sets `route_open=true`,
  `route_start`); expand toggle.
- **Expanded:** Allegiance (full name), System (stellar list + GG/belt counts +
  OtherWorlds), UWP (each glyph + blurb), {Ix}, (Ex), [Cx], Population
  (TotalPopulation + sophont remarks), Nobility, Remarks, Travel Zone + TAS
  rating; buttons: Print, J-1…J-6 (call planner `do_route` with this world as
  start), Generate World Map (outbound link).
- **Print:** reuse the Blob-URL self-printing pattern (`route_print.rs`) → new
  `crates/frontend/src/world_print.rs` mirroring `print/world.html`.
- **Thumbnail (recommended):** the `res/Candy/` hydrographics generics
  (`Hyd0.png`–`HydA.png`, `Belt.png`) via `/api/res/Candy/...` — the reference's
  own fallback. Skip S3 + star compositing for v1.

## 5. Critical files + phased order
- **Phase A (core decoders):** new `crates/core/src/world_util.rs` + `lib.rs`
  `pub mod`; unit tests (Regina, an Aslan world, a belt). Runs under `cargo test`.
- **Phase B (selection + full fetch, `main.rs`):** extract `nearest_world`; add
  `selected` + `full_sectors` + `select_world`; branch `on_up` on `route_open`.
- **Phase C (UI):** `world_panel.rs`, `world_print.rs`; `mod` decls + render
  `<WorldPanel>`; pass `selected` + planner signals/`do_route`.
- **Phase D (polish):** subsector-letter helper, sophont-code table, placeholder
  `XXXXXXX-X` handling, empty-field hiding.

## 6. Verification
`cargo test -p tmap-core` (table tests vs known outputs: Regina `A788899-C` →
"Excellent", `ImDd` → "Third Imperium, Domain of Deneb", zone `R` → Red, etc.);
`cargo check -p tmap-frontend --target wasm32-unknown-unknown`; manual end-to-end
(click Regina → panel; expand → all sections; one `?lod=full` request cached per
sector; wiki/map-gen links; J-3 seeds planner; Print tab; ✕/empty-click closes;
route mode still sets endpoints). Diff each decoded line vs travellermap.com.
