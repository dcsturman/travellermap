# Implementation Plan: Map Style Themes (Poster / Atlas / Print / Candy + reference presets)

> Source: planning pass 2026-06-18 against `server/Stylesheet.cs` and the frontend
> `crates/frontend/src/render/` module. Implements PORT_PLAN's "Style themes" item.
> **Plan only — no code written yet.**

## 1. The reference spec, distilled

The C# `Stylesheet` (`server/Stylesheet.cs`) is a single object constructed once per
render from `(double scale, MapOptions options, Style style)`. Its constructor does two
things in sequence:

1. **Scale-driven LOD setup** (lines 255–541): visibility flags, font sizes, positions,
   pen widths — all functions of `scale`. **Already ported** into
   `crates/frontend/src/render/common.rs` thresholds + per-pass code. Themes do NOT touch
   this.
2. **A `switch (style)` block** (lines 543–1058) that overrides a comparatively small set
   of palette/font/flag fields per preset, followed by a `DefaultTo(...)` cascade
   (1060–1095) that fills any unset color from five "generic" colors: `foregroundColor`,
   `lightColor`, `darkColor`, `dimColor`, `highlightColor`.

**Key insight: a theme is almost entirely (a) the five generic colors, (b) ~15 element
colors, (c) a font-family override string, and (d) a handful of boolean/enum flags.**
Everything geometric is shared. The theme is a *small palette+flags struct*, not a
re-parameterization of the whole renderer.

`Style` enum (line 213): `Poster, Atlas, Candy, Print, Draft, FASA, Terminal, Mongoose`.
`style=` selects one; `Poster` is the default (empty case, 545–548).

### Keep vs. skip for client-side rendering
- **Skip:** `preferredMimeType` (image-only), `useWorldImages` (Candy S3 globe textures —
  already DECIDED out), PDF concerns, `FontCache`/`AbstractFont` plumbing,
  `HighlightWorldPattern` (the `hilite=` feature, not a theme).
- **Keep:** every color, font-family overrides, `grayscale`/`lightBackground`, zone pen
  colors/widths, `showGalaxyBackground`/`showNebulaBackground`/`deepBackgroundOpacity`/
  `riftOpacity` caps, `hexStyle`/`microBorderStyle`, `numberAllHexes`, world-disc
  water/no-water fills, `discRadius`, world-glyph layout overrides (Mongoose), text-case
  flags, and the `worldDetails &= ~...` masks that drop fields per theme.

## 2. Proposed `Theme` data structure

Start in a new `crates/frontend/src/render/theme.rs` beside `common.rs` (plain data, no
Leptos/web-sys deps, so it can move to `tmap-core` later if a precompute tool needs it).

```rust
pub struct Theme {
    // generic cascade colors (Stylesheet foreground/light/dark/dim/highlight)
    pub fg: Color, pub light: Color, pub dark: Color, pub dim: Color, pub highlight: Color,
    // backgrounds
    pub background: Color, pub light_background: bool, pub grayscale: bool,
    pub show_galaxy: bool, pub show_nebula: bool,
    pub rift_opacity_cap: f64, pub deep_bg_zero: bool,
    // element colors
    pub macro_border: Color, pub micro_border: Option<Color>, // None ⇒ otu.css per-allegiance
    pub macro_route: Color, pub micro_route: Option<Color>,
    pub micro_border_text: Color,
    pub amber_zone: Color, pub red_zone: Color, pub green_zone: Option<Color>,
    pub grid: Option<Color>, // FASA/Mongoose force; None ⇒ scale-faded gray
    pub capital_fill: Color, pub capital_text: Color,
    pub world_water: Color, pub world_no_water: Color, pub world_no_water_outline: Option<Color>,
    pub star_field: Color, // defaults to fg
    // world detail color mode
    pub world_detail_colors: bool, // Atlas/FASA force OFF; Poster respects the toggle
    // fonts
    pub font_family: &'static str, pub mono_like: bool,
    // text case + hex flags
    pub uppercase_worlds: bool, pub uppercase_sector_names: bool,
    pub uppercase_micro_border_names: bool,
    pub number_all_hexes: bool, pub hex_coord_subsector: bool,
    pub hex_style_none: bool, pub micro_border_curve: bool, pub fade_sector_names: bool,
    // world glyph layout overrides (Mongoose)
    pub world_layout: WorldLayout, // { Standard, Mongoose }
    // per-theme worldDetails removals
    pub drop_starport: bool, pub drop_allegiance: bool, pub drop_bases: bool,
    pub drop_gas_giant: bool, pub drop_uwp: bool, pub drop_highlight: bool,
    pub name: &'static str, // "Poster" — URL param + UI label
}
```

`Color` = a newtype wrapping a CSS string (canvas takes `&str`). `Color::EMPTY` models
C#'s `Color.Empty`/`IsEmpty` so the `DefaultTo` cascade ports as
`if c.is_empty() { c = fallback }`. A free `theme(style) -> Theme` builds each preset by
starting from `Theme::poster()` and overriding fields — mirroring the C# `switch`.

## 3. The eight presets — per-preset differences (cited from Stylesheet.cs)

| Field | Poster (default) | Atlas | Print | Candy | Draft | FASA | Terminal | Mongoose |
|---|---|---|---|---|---|---|---|---|
| `background` | Black `#000` | White (565) | White (671) | Black | AntiqueWhite (705) | White (601) | Black (866) | `#e6e7e8` (966) |
| `fg` | White (535) | Black (564) | Black (670) | White | Black α0xB0 (706) | ink `#5C4033` (600) | Cyan (867) | Black (967) |
| `highlight` | Red (539) | Gray (569) | Red | Red | Red α0xB0 (707) | ink (626) | White (868) | Red (968) |
| `light/dark/dim` | LightGray/DarkGray/DimGray | DarkGray/DarkGray/LightGray | DarkGray/DarkGray/LightGray | (default) | DarkCyan/Black/Black α (709-711) | ink (623-625) | LightBlue/DarkBlue/DimGray (870) | Black/Black/Gray (970) |
| `grayscale` | false | **true** (552) | false | false | false | **true** (603) | false | false |
| `light_background` | false | **true** (553) | true (668) | false | true (701) | true (604) | false | true (953) |
| `show_galaxy` | true | true | true | true | **false** (700) | **false** (594) | **false** (863) | **false** (952) |
| `rift_opacity_cap` | none | 0.70 (577) | 0.70 (683) | always | 0.30 (775) | 0 (596) | 0.30 (941) | 0.30 (1037) |
| `macro_border` | Red (398) | Black (559) | Red | (default) | (default) | ink (613) | (default) | (default) |
| `micro_border` | Gray (400) | Black (561) | Gray | Red α128 (845) | Brown-ish | ink (616) | Gray | (default) |
| `micro_route` | Gray (401) | Gray (562) | Gray (675) | (default) | Gray (770) | ink (621) | Gray (936) | (default) |
| `micro_border_text` | Amber (405) | Gray (570) | **Brown** (677) | — | Brown α (773) | ink (627) | Cyan (939) | DarkSlateGray (976) |
| `world_water` | DeepSkyBlue (406) | Black→Empty (571) | (default) | (default) | Empty/outline (763) | ink (635) | Empty/outline (929) | **MediumBlue** (1023) |
| `world_no_water` | White (407) | White + black pen (574) | White + black pen (680) | (default) | fg fill (762) | ink (636) | fg fill (928) | **DarkKhaki** (1024) |
| `world_detail_colors` | toggle (302) | **off** (579) | toggle | toggle | toggle | **off** (640) | toggle | toggle |
| `font_family` | Arial | Arial | Arial | Arial | **Comic Sans MS** (716) | Arial | **Courier New** (876) | **Calibri,Arial** (978) |
| `uppercase_worlds` | false | false | false | **true** (853) | **true** (746) | false | **true** (916) | **true** (1013) |
| `number_all_hexes` | false | false | false | (n/a) | **true** (777) | **true** (651) | **true** (943) | false |
| `green_zone` | none | none | none | none | none | none | none | **visible** (1028) |
| zone colors | Amber/Red | LightGray/Black (557) | Amber/Red | Goldenrod (825) | fg/fg (766) | ink (608) | fg (932) | green/amber/red (1031-1033) |
| `drop_*` | none | none | none | starport/alleg/bases/hex (818) | allegiance (752) | starport/alleg/bases/GG/highlight/uwp (642-647) | none | allegiance (1015) |
| world layout | Standard | Standard | Standard | Standard | Standard | Standard | Standard | **Mongoose** (1039-1054) |

**The four UI thumbnails (PORT_PLAN) are Poster / Atlas / Print / Candy.** Best MVP set:
Poster (no-op default), **Atlas** (grayscale, white bg, black ink — cleanest dramatic
alternate), **Print** (white bg, color worlds, brown border text). Candy is heaviest
(nebula textures, curve borders, world images — much DECIDED out), so treat as
"approximate" or defer.

## 4. Refactor map — hardcoded style constants → theme fields

**`render/common.rs`** (central palette):
- `C_BORDER`/`C_RED` (48,52), `C_ROUTE` (49), `C_RIFT` (50), `C_AMBER` (51), `C_WATER`
  (53), `C_DRY` (54) → `theme.macro_border`, `theme.micro_route`/`macro_route`, rift =
  `theme.fg` α + `rift_opacity_cap`, `theme.amber_zone`, `theme.world_water`,
  `theme.world_no_water`.
- `DEFAULT_FONT` (59) → `theme.font_family`.
- `grid_color()` (342–345) → respect `theme.grid` when `Some`, else scale-faded gray.
- `allegiance_border_color()` (349–380) — the otu.css port. Stays for Poster/Print;
  Atlas/FASA/Terminal force all micro borders to `theme.micro_border`. Model via
  `theme.micro_border: Option<Color>` (None ⇒ per-allegiance cascade).

**`render/mod.rs`**: `c.clear("#000000", …)` (114, jump-cutout 110) → `theme.background`.
`JUMPMAP_SURROUND` (49) stays (jump-map chrome).

**`render/worlds.rs`**:
- `world_colors()` (423–463) trade-class palette — skipped when `world_detail_colors` is
  forced off (Atlas/FASA); plain water/dry → `theme.world_water`/`world_no_water`; vacuum
  outline → `theme.world_no_water_outline`.
- Disc/glyph fills `#ffffff` (108,139), `#e9eef9` (293,347,356), `#9aa3b8` (285),
  `#cfd6e6` (318,321), `#c9d2e4` (368), `#aab3c8` (381) → `theme.fg`/`light`/`dim`.
  `C_RED` highlight (347,356) → `theme.highlight`. Capital `#e8636f` (402) →
  `theme.capital_text`.
- `Georgia` (85)/`Arial Unicode MS` (86) placeholder/anomaly fonts are theme-independent —
  leave.
- Add `uppercase_worlds` handling here. Font *sizes* are scale-driven — leave; only the
  family flows from the theme.

**`render/grid.rs`**: thread theme so FASA/Mongoose forced grid colors apply.

**`render/labels.rs`**: sector-name fade `#ffffff`/`#a9a9a9`/`#696969` (28,30,32) →
`theme.fg`/`dark`/`dim` (the `fadeSectorSubsectorNames` cascade 1061–1067; add
`theme.fade_sector_names`). Compass `COLOR` (189) = UI chrome, leave. Border-label amber
`C_AMBER` (111) → `theme.micro_border_text`.

**`render/overlays.rs`**: macro names white/`C_RED` (97,99,123,125), capital wheat
`#f5deb3` (202), label `#e8636f` (220) → `theme.fg`/`highlight`/`capital_fill`. Rift fill
→ `theme.fg` α (skip when `rift_opacity_cap == 0`, e.g. FASA).

**`render/routes.rs`**: per-allegiance route palette (15–21) overridden when theme forces
`micro_route`; else otu.css. Jump-route planner cyan (62–63,66) = UI chrome, leave.

**`render/stars.rs`**: star tint `rgba(...)` (78–81) → from `theme.star_field` (defaults
to fg). Gate galaxy image on `theme.show_galaxy`.

**`render/status.rs`** / **`render/hud.rs`**: dim overlay + debug HUD = theme-independent,
leave.

## 5. Selection, threading, persistence, sharing

- **Threading:** add `theme: &Theme` to `render::draw` (mod.rs:61) and pass to every pass
  (they already take `&c, &view, w, h, …`; add `theme`). Mechanical, low-risk — the
  render-module file split was done for exactly this.
- **Selection signal:** `let style = RwSignal::new(...)` near the `opt_*` signals
  (main.rs ~860). In the redraw `Effect` (1108) compute `let theme = render::theme(style.get());`
  and pass `&theme` into `render::draw` (1155). Reading `style.get()` subscribes the
  effect → switching redraws.
- **Cache invalidation:** `borders.rs` bakes resolved color into the per-group
  `SectorGroup` cache (129/182). A theme switch must invalidate it — call
  `render::clear_caches()` on switch (cheap), or key the border cache on theme name. Grid/
  dot caches apply color at draw — verify each; safest is `clear_caches()` on theme change.
- **UI:** in the settings panel (~2579) add a theme selector above the toggles (reuse the
  milieu-selector pattern). Remove "style themes" from the "Not yet ported" note (now just
  text already trimmed).
- **Persistence/sharing:** add `&style=<name>` to the URL. `build_share_url` (640) append
  when `style != Poster` (mirror the milieu guard at 648); `parse_share_params` (657) read
  `params.get("style")` → `Style` enum; seed the signal on load (like `url_milieu` at 735);
  extend the debounced `replaceState` effect (781) to read `style.get()`. This is
  travellermap.com's own param name (`style=poster|atlas|print|candy|…`) — forward-compatible
  with the future URL-compat work.

## 6. Phased rollout (smallest demoable first)

- **Phase A — Theme plumbing + default extraction (no visible change).** Define `Theme` +
  `Theme::poster()` from the *current* hardcoded values, thread `&Theme` through
  `render::draw` and every pass, replace `common.rs` consts / inline literals with theme
  reads. **Demo:** map looks pixel-identical (verify with a screenshot diff). This is the
  load-bearing refactor.
- **Phase B — Atlas.** Add `Theme::atlas()` (white bg, grayscale, black ink) — biggest
  contrast; exercises `background`, the `fg`/`dark`/`dim` cascade, forced micro-border,
  `world_detail_colors=off`, fade colors. **Demo:** same map as a clean black-on-white
  atlas — proves the cascade end-to-end.
- **Phase C — Selection + persistence.** Add the `style` signal, settings-panel selector
  (Poster/Atlas), URL `&style=` round-trip, cache invalidation. **Demo:** flip
  Poster↔Atlas in the UI; survives reload; in the shareable link.
- **Phase D — Full set.** Print, Draft, FASA, Terminal, Mongoose (Mongoose adds
  `WorldLayout::Mongoose` glyph-position override — the only preset needing `worlds.rs`
  layout changes). Candy last/approximate (skip nebula/world-images/curve borders;
  best-effort = dark + goldenrod zones + uppercase + red-α borders). Wire all eight into
  the selector with thumbnails.

## 7. Risks / unknowns

- **Browser fonts.** Reference bundles Windows fonts: Comic Sans MS (Draft), Courier New
  (Terminal), Calibri (Mongoose — already `"Calibri,Arial"` fallback at 978), Georgia
  (placeholder). Comic Sans/Courier/Georgia broadly available; Calibri Windows-only → rely
  on the fallback. Wingdings already replaced by our `glyph.rs` Unicode table. Exact
  typeface parity is best-effort.
- **`worldDetails &= ~...` masks.** FASA/Candy/Draft/Mongoose drop fields (starport,
  allegiance, bases, gas-giant, UWP). `worlds.rs` `draw_world_glyphs` draws these
  unconditionally within scale bands — the `drop_*` flags must gate each (they're in
  separate state-batched passes, so independently skippable).
- **Border-color caching** — see §5; must not be forgotten or borders keep the old palette.
- **Per-sector `otu.css` cascade (`SectorStylesheet`)** is *orthogonal* and already handled
  (PORT_PLAN Phase 7: border `Color` attr → sector `<Stylesheet>` → otu.css → gray, at
  `borders.rs:129`). The theme sits **above** it: theme-forced-color ?? sector-cascade ??
  otu.css ?? gray. No new SectorStylesheet work needed.
- **`Color.Empty`/`DefaultTo` semantics.** Some fields are left `Empty` so the cascade
  fills them (e.g. `worldNoWater.pen.color = Empty` = "no outline"). The `Color` newtype
  must distinguish "empty/no-paint" from a real color or Atlas black-pen/no-pen renders
  wrong.
- **Candy scope.** `useWorldImages`, `showNebulaBackground`, `microBorderStyle=Curve`, and
  relative-scale pen tweaks are substantial and partly DECIDED-out. Scope Candy as
  "approximate" or defer so it doesn't block the other seven.

## Critical files
- `server/Stylesheet.cs` — the spec; `switch(style)` (543–1058) + `DefaultTo` (1060–1095).
- `crates/frontend/src/render/common.rs` — palette consts + `grid_color`/`allegiance_border_color`; home of the `Theme` struct (or new `render/theme.rs`).
- `crates/frontend/src/render/mod.rs` — `draw()` orchestrator; add + thread the `theme` param.
- `crates/frontend/src/render/worlds.rs` — `world_colors()` + disc/glyph literals; only file needing layout changes (Mongoose).
- `crates/frontend/src/main.rs` — `style` signal, settings-panel selector (~2579), `build_share_url`/`parse_share_params` (640/657), redraw effect (1108).
