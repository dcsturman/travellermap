//! Map style themes — the palette/font/flags selected per preset, threaded through
//! every render pass. Ported **verbatim** from the reference `server/Stylesheet.cs`
//! `switch (style)` block + its `DefaultTo` cascade. See `STYLE_THEMES_PLAN.md`.
//!
//! Structure mirrors the reference: `poster()` is the default (our current look,
//! kept as-is), and each alternate is `Self { <overrides>, ..Self::poster() }` —
//! the same "override from default" shape as the C# cases. Alternate colors come
//! from each preset's cascade (`foregroundColor`/`lightColor`/`darkColor`/
//! `dimColor`/`highlightColor` + element overrides); the `DefaultTo` rules
//! (`Stylesheet.cs:1069-1095`) tell which cascade color each glyph uses:
//! world text ← foreground, hex# ← light, highlights ← highlight, stars ← foreground.
//!
//! .NET `Color` names → hex are the standard `KnownColor` ARGB values.
//! `TravellerColors.Red = #E32736`, `Amber = #FFCC00`.
//!
//! **Not yet replicated** (flagged per preset; colors/fonts/case/detail-drops are
//! faithful): curved micro borders (FASA/Candy), all-hex numbering + subsector hex
//! coords (Draft/FASA/Terminal), the Mongoose glyph re-layout + zone-perimeters +
//! filled-UWP, and text scale-expansion. Candy is deferred entirely (needs world
//! globe images + nebula background, out of our renderer's scope).

use super::common::{C_AMBER, C_BORDER, C_DRY, C_RED, C_RIFT, C_ROUTE, C_WATER};

/// A render style preset. `Copy` (all fields `&'static str`/small) so it threads
/// cheaply into each pass.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Preset name (for the upcoming `&style=` share-URL round-trip).
    #[allow(dead_code)]
    pub name: &'static str,
    /// Canvas clear color (`backgroundColor`).
    pub background: &'static str,
    /// Font family stack for map text (`*.fontInfo.families`); poster = Arial stack.
    pub font: &'static str,

    // ── macro overlays ──
    pub macro_border: &'static str, // macroBorders.pen.color
    pub route: &'static str,        // macroRoutes.pen.color
    pub rift: &'static str,         // rift fill
    pub show_galaxy: bool,          // showGalaxyBackground
    pub show_rift: bool,            // riftOpacity > 0
    // micro (per-sector) borders/routes: None ⇒ the per-allegiance otu.css cascade;
    // Some ⇒ a single forced color (Atlas/FASA borders; Atlas/Print/Draft/FASA/Terminal routes).
    pub micro_border: Option<&'static str>, // microBorders.pen.color
    pub micro_route: Option<&'static str>,  // microRoutes.pen.color
    pub micro_border_text: &'static str,    // microBorders.textColor (region/border labels)

    // ── zones / highlight ──
    pub amber: &'static str,             // amberZone.pen.color
    pub red_zone: &'static str,          // redZone.pen.color
    pub highlight: &'static str,         // highlightColor (anomaly glyph, highlighted names, minor region names)
    // Mongoose green-zone perimeter color. Set per `Stylesheet.cs` but not yet drawn
    // (Mongoose's `showZonesAsPerimeters` — perimeter rings for every world — isn't
    // replicated; flagged in the module docs).
    #[allow(dead_code)]
    pub green_zone: Option<&'static str>,

    // ── world discs ──
    pub world_water: &'static str,             // worldWater.fillColor
    pub world_dry: &'static str,               // worldNoWater.fillColor (also placeholder dot/glyph)
    pub world_dry_outline: Option<&'static str>, // worldNoWater.pen.color
    pub vacuum_fill: &'static str,             // our black-disc vacuum world
    pub force_plain_worlds: bool,              // showWorldDetailColors == false
    // trade-class detail tints (fixed in RenderContext.WorldColors — not the cascade)
    pub ag_green: &'static str,
    pub rich_purple: &'static str,
    pub ind_gray: &'static str,
    pub exotic_rust: &'static str,

    // ── world text/glyphs (poster keeps custom tints; alternates use the cascade) ──
    pub text: &'static str,       // worlds.textColor ← foreground
    pub text_hex: &'static str,   // hexNumber.textColor ← light
    pub text_gg: &'static str,    // gas giant ← foreground
    pub text_uwp: &'static str,   // uwp.textColor ← foreground
    pub text_alleg: &'static str, // allegiance ← foreground
    pub uppercase_worlds: bool,   // worlds.textStyle.Uppercase

    // worldDetails drops (`worldDetails &= ~…`)
    pub drop_starport: bool,
    pub drop_bases: bool,
    pub drop_gas_giant: bool,
    pub drop_uwp: bool,
    pub drop_allegiance: bool,
    pub drop_highlight: bool, // suppress the red highlight on names (FASA)

    // ── capitals / homeworlds (Worlds.xml) ──
    pub capital: &'static str,      // capitals.textColor
    pub capital_fill: &'static str, // capitals.fillColor

    // ── macro / mega region names ──
    pub macro_name: &'static str, // macroNames.textColor ← foreground
    pub mega_name: &'static str,  // megaNames.textColor ← foreground

    // ── sector/subsector watermark names (fade cascade; foreground/dark/dim) ──
    pub name_full: &'static str,
    pub name_dark: &'static str,
    pub name_dim: &'static str,

    // ── grids ──
    pub grid: Option<&'static str>, // pen color override; None ⇒ scale-faded gray (gridColor)

    // ── star field (4 brightness tiers; ← foreground) ──
    pub stars: [&'static str; 4],
}

impl Theme {
    /// **Poster** (default) — today's exact values. Reference-named colors come from
    /// the `common` consts; the world-text tints are our custom values (kept per the
    /// "keep current Poster" decision rather than re-based to the reference White cascade).
    pub fn poster() -> Self {
        Self {
            name: "Poster",
            background: "#000000",
            font: "Arial, 'Helvetica Neue', Helvetica, sans-serif",
            macro_border: C_BORDER,
            route: C_ROUTE,
            rift: C_RIFT,
            show_galaxy: true,
            show_rift: true,
            micro_border: None,
            micro_route: None,
            micro_border_text: C_AMBER,
            amber: C_AMBER,
            red_zone: C_RED,
            highlight: C_RED,
            green_zone: None,
            world_water: C_WATER,
            world_dry: C_DRY,
            world_dry_outline: None,
            vacuum_fill: "#000000",
            force_plain_worlds: false,
            ag_green: "#048104",
            rich_purple: "#a000a0",
            ind_gray: "#888888",
            exotic_rust: "#cc6626",
            text: "#e9eef9",
            text_hex: "#9aa3b8",
            text_gg: "#cfd6e6",
            text_uwp: "#c9d2e4",
            text_alleg: "#aab3c8",
            uppercase_worlds: false,
            drop_starport: false,
            drop_bases: false,
            drop_gas_giant: false,
            drop_uwp: false,
            drop_allegiance: false,
            drop_highlight: false,
            capital: "#e8636f",
            capital_fill: "#f5deb3",
            macro_name: "#ffffff",
            mega_name: "rgba(255,255,255,0.92)",
            name_full: "#ffffff",
            name_dark: "#a9a9a9",
            name_dim: "#696969",
            grid: None,
            stars: [
                "rgba(170,180,205,0.35)",
                "rgba(205,215,235,0.55)",
                "rgba(230,235,250,0.75)",
                "rgba(255,255,255,0.9)",
            ],
        }
    }

    /// **Atlas** — grayscale on white (`Stylesheet.cs:550-590`). fg=Black, light/dark=
    /// DarkGray, dim=LightGray, highlight=Gray; borders/zones black/gray; world detail
    /// colors off; dry worlds white + black outline.
    pub fn atlas() -> Self {
        Self {
            name: "Atlas",
            background: "#ffffff",
            macro_border: "#000000",       // Black
            route: "#808080",              // Gray
            micro_border: Some("#000000"), // microBorders Black
            micro_route: Some("#808080"),  // microRoutes Gray
            micro_border_text: "#808080",  // microBorders.textColor Gray
            amber: "#d3d3d3",              // amberZone LightGray
            red_zone: "#000000",           // redZone Black
            highlight: "#808080",          // Gray
            world_water: "#000000",        // worldWater Black (then overridden: noWater white+black pen)
            world_dry: "#ffffff",          // worldNoWater White
            world_dry_outline: Some("#000000"), // worldNoWater.pen Black
            force_plain_worlds: true,      // showWorldDetailColors = false
            vacuum_fill: "#000000",
            // cascade text → fg=Black, hex#←light=DarkGray
            text: "#000000",
            text_hex: "#a9a9a9",
            text_gg: "#000000",
            text_uwp: "#000000",
            text_alleg: "#000000",
            capital: "#000000",            // capitals.textColor Black
            capital_fill: "#a9a9a9",       // capitals.fillColor DarkGray
            macro_name: "#000000",
            mega_name: "#000000",
            name_full: "#000000",          // fade: fg=Black / dark=DarkGray / dim=LightGray
            name_dark: "#a9a9a9",
            name_dim: "#d3d3d3",
            stars: ["#000000", "#000000", "#000000", "#000000"], // ← foreground
            ..Self::poster()
        }
    }

    /// **Print** — color on white (`Stylesheet.cs:666-694`). fg=Black, light/dark=
    /// DarkGray, dim=LightGray; micro routes gray; border text Brown; world detail
    /// colors ON (trade tints kept); dry worlds white + black outline; highlight stays Red.
    pub fn print() -> Self {
        Self {
            name: "Print",
            background: "#ffffff",
            micro_route: Some("#808080"),  // microRoutes Gray
            micro_border_text: "#a52a2a",  // microBorders.textColor Brown
            world_dry: "#ffffff",
            world_dry_outline: Some("#000000"), // worldNoWater.pen Black
            amber: C_AMBER,                // amberZone TravellerColors.Amber (explicit)
            // Print does NOT set highlightColor/redZone/macroBorders → stay default Red.
            text: "#000000",
            text_hex: "#a9a9a9",           // ← light=DarkGray
            text_gg: "#000000",
            text_uwp: "#000000",
            text_alleg: "#000000",
            capital_fill: "#f5deb3",       // unchanged (Wheat)
            macro_name: "#000000",         // ← foreground
            mega_name: "#000000",
            name_full: "#000000",
            name_dark: "#a9a9a9",
            name_dim: "#d3d3d3",
            stars: ["#000000", "#000000", "#000000", "#000000"],
            ..Self::poster()
        }
    }

    /// **Draft** — blueprint look (`Stylesheet.cs:696-788`). AntiqueWhite bg, ink at
    /// 0xB0 opacity (black@B0 fg, red@B0 highlight, DarkCyan@B0 light); Comic Sans;
    /// uppercase worlds; allegiance dropped; rift ≤0.30; numberAllHexes (not replicated).
    pub fn draft() -> Self {
        Self {
            name: "Draft",
            background: "#faebd7",         // AntiqueWhite
            font: "'Comic Sans MS', 'Comic Sans', cursive",
            show_galaxy: false,
            highlight: "rgba(227,39,54,0.69)", // red@0xB0
            world_water: "rgba(0,0,0,0.69)",   // worldWater empty → pen=fg; we fill with fg
            world_dry: "rgba(0,0,0,0.69)",     // worldNoWater = foreground
            amber: "rgba(0,0,0,0.69)",         // amberZone = foreground
            uppercase_worlds: true,
            drop_allegiance: true,
            micro_route: Some("#808080"),       // microRoutes Gray
            micro_border_text: "rgba(165,42,42,0.69)", // microBorders.textColor Brown@B0
            grid: Some("rgba(0,139,139,0.69)"), // parsecGrid = lightColor (DarkCyan@B0)
            text: "rgba(0,0,0,0.69)",          // fg = black@B0
            text_hex: "rgba(0,139,139,0.69)",  // ← light = DarkCyan@B0
            text_gg: "rgba(0,0,0,0.69)",
            text_uwp: "rgba(0,0,0,0.69)",
            text_alleg: "rgba(0,0,0,0.69)",
            capital: "rgba(0,0,0,0.69)",
            capital_fill: "#f5deb3",
            macro_name: "rgba(0,0,0,0.69)",
            mega_name: "rgba(0,0,0,0.69)",
            name_full: "rgba(0,0,0,0.69)",
            name_dark: "rgba(0,0,0,0.69)",     // dark = black@B0
            name_dim: "rgba(0,0,0,0.345)",     // dim = black@B0/2
            stars: ["rgba(0,0,0,0.69)"; 4],
            ..Self::poster()
        }
    }

    /// **FASA** — sepia line-art (`Stylesheet.cs:592-664`). White bg, ink `#5C4033`
    /// everywhere, grayscale, no galaxy/rifts; micro border text ink, regular weight;
    /// drops starport/allegiance/bases/gas-giant/highlight/uwp; numberAllHexes +
    /// curved borders + subsector hex coords (not replicated).
    pub fn fasa() -> Self {
        Self {
            name: "FASA",
            background: "#ffffff",
            show_galaxy: false,
            show_rift: false,              // riftOpacity = 0
            macro_border: "#5c4033",
            route: "#5c4033",
            micro_border: Some("#5c4033"),
            micro_route: Some("#5c4033"),
            micro_border_text: "#5c4033",
            amber: "#5c4033",
            red_zone: "rgba(92,64,51,0.5)", // redZone pen empty → fill ink@0x80
            highlight: "#5c4033",
            world_water: "#5c4033",
            world_dry: "#5c4033",
            force_plain_worlds: true,
            uppercase_worlds: false,
            drop_starport: true,
            drop_bases: true,
            drop_gas_giant: true,
            drop_uwp: true,
            drop_allegiance: true,
            drop_highlight: true,
            grid: Some("rgba(92,64,51,0.5)"), // grids = lightColor (ink@0x80)
            text: "#5c4033",
            text_hex: "rgba(92,64,51,0.5)",   // ← light = ink@0x80
            text_gg: "#5c4033",
            text_uwp: "#5c4033",
            text_alleg: "#5c4033",
            capital: "#5c4033",
            capital_fill: "#5c4033",
            macro_name: "#5c4033",
            mega_name: "#5c4033",
            name_full: "#5c4033",
            name_dark: "#5c4033",
            name_dim: "#5c4033",
            stars: ["#5c4033"; 4],
            ..Self::poster()
        }
    }

    /// **Terminal** — green/cyan CRT (`Stylesheet.cs:860-948`). Black bg, Cyan fg,
    /// White highlight, LightBlue/DarkBlue/DimGray cascade; Courier New; uppercase;
    /// micro routes gray; subsector grid Cyan, parsec grid Plum; rift ≤0.30.
    pub fn terminal() -> Self {
        Self {
            name: "Terminal",
            background: "#000000",
            font: "'Courier New', 'Courier', monospace",
            show_galaxy: false,
            highlight: "#ffffff",          // White
            world_water: "#00ffff",        // empty → pen=fg (Cyan); we fill with fg
            world_dry: "#00ffff",          // worldNoWater = foreground (Cyan)
            amber: "#00ffff",              // amberZone = foreground
            uppercase_worlds: true,
            micro_route: Some("#808080"),  // microRoutes Gray
            micro_border_text: "#00ffff",  // microBorders.textColor Cyan
            grid: Some("#00ffff"),         // subsectorGrid Cyan (parsec Plum not separately modeled)
            text: "#00ffff",               // fg = Cyan
            text_hex: "#add8e6",           // ← light = LightBlue
            text_gg: "#00ffff",
            text_uwp: "#00ffff",
            text_alleg: "#00ffff",
            capital: "#00ffff",
            capital_fill: "#00ffff",
            macro_name: "#00ffff",
            mega_name: "#00ffff",
            name_full: "#00ffff",          // fadeSectorSubsectorNames=false → flat fg (Cyan)
            name_dark: "#00ffff",
            name_dim: "#00ffff",
            stars: ["#00ffff"; 4],
            ..Self::poster()
        }
    }

    /// **Mongoose** — the boxed Mongoose-rulebook look (`Stylesheet.cs:950-1056`).
    /// `#e6e7e8` bg, Black fg, Red highlight, Black/Black/Gray cascade; Calibri,Arial;
    /// uppercase; allegiance dropped; MediumBlue/DarkKhaki worlds + DarkGray outline;
    /// green/amber/red zone perimeters; grids black; border text DarkSlateGray.
    /// Glyph re-layout + zone-perimeters + filled-UWP background are **not replicated**.
    pub fn mongoose() -> Self {
        Self {
            name: "Mongoose",
            background: "#e6e7e8",
            font: "Calibri, Arial, sans-serif",
            show_galaxy: false,
            highlight: "#ff0000",          // Red (System.Red here, not TravellerColors)
            green_zone: Some("#80c676"),
            amber: "#fbb040",
            red_zone: "#ff0000",
            world_water: "#0000cd",        // MediumBlue
            world_dry: "#bdb76b",          // DarkKhaki
            world_dry_outline: Some("#a9a9a9"), // worldWater/NoWater pen DarkGray
            uppercase_worlds: true,
            drop_allegiance: true,
            micro_border_text: "#2f4f4f",  // microBorders.textColor DarkSlateGray
            grid: Some("#000000"),         // all grids = foreground (Black)
            text: "#000000",               // fg = Black
            text_hex: "#000000",           // ← light = Black
            text_gg: "#000000",
            text_uwp: "#000000",
            text_alleg: "#000000",
            capital: "#000000",
            capital_fill: "#f5deb3",
            macro_name: "#000000",
            mega_name: "#000000",
            name_full: "#000000",          // fade: fg=Black / dark=Black / dim=Gray
            name_dark: "#000000",
            name_dim: "#808080",
            stars: ["#000000"; 4],
            ..Self::poster()
        }
    }

    /// All selectable presets, in UI order (Candy deferred — see module docs).
    #[allow(clippy::type_complexity)]
    pub const PRESETS: &'static [(&'static str, fn() -> Theme)] = &[
        ("Poster", Theme::poster),
        ("Atlas", Theme::atlas),
        ("Print", Theme::print),
        ("Draft", Theme::draft),
        ("FASA", Theme::fasa),
        ("Terminal", Theme::terminal),
        ("Mongoose", Theme::mongoose),
    ];

    /// Resolve a preset by name (case-insensitive); unknown → Poster.
    pub fn from_name(name: &str) -> Theme {
        Self::PRESETS
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, f)| f())
            .unwrap_or_else(Self::poster)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::poster()
    }
}
