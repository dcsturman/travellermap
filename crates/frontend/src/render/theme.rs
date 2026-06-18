//! Map style theme — the palette + font selected per preset, threaded through
//! every render pass so an alternate preset (Atlas, Print, …) only swaps values.
//!
//! Phase A of `STYLE_THEMES_PLAN.md`: the `Poster` (default) theme carries today's
//! exact colors/font, so threading it through changes nothing on screen — it's the
//! load-bearing refactor that the later presets build on. Each field documents the
//! reference `Stylesheet` concept it maps to. (Borders/routes/grid still use their
//! per-allegiance otu.css cascades for now; those become theme overrides in a later
//! phase, alongside the border-cache invalidation that a color switch needs.)

use super::common::{C_AMBER, C_BORDER, C_DRY, C_RED, C_RIFT, C_ROUTE, C_WATER};

/// A render style preset: palette + font. `Copy` (all fields are `&'static str` /
/// small) so it threads cheaply by value-or-ref into each pass.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Canvas clear color (Stylesheet `backgroundColor`).
    pub background: &'static str,
    // (font family is deferred to the later font-varying presets — Draft/Terminal/
    //  Mongoose; Poster/Atlas/Print all render in the default Arial stack.)

    // ── macro overlays ──
    pub macro_border: &'static str, // C_BORDER
    pub route: &'static str,        // C_ROUTE
    pub rift: &'static str,         // C_RIFT

    // ── zones / highlight ──
    pub amber: &'static str, // C_AMBER (amber travel zone)
    pub red: &'static str,   // C_RED (red zone, highlighted names, minor region names)

    // ── world discs ──
    pub world_water: &'static str, // C_WATER (DeepSkyBlue)
    pub world_dry: &'static str,   // C_DRY (white) — also the placeholder dot/glyph + vacuum outline
    pub vacuum_fill: &'static str,  // black disc behind the white vacuum outline
    // trade-class detail tints (Stylesheet WorldColors)
    pub ag_green: &'static str,
    pub rich_purple: &'static str,
    pub ind_gray: &'static str,
    pub exotic_rust: &'static str,

    // ── world text/glyph tints (our derived fg/dim set) ──
    pub text: &'static str,       // primary world text (name, starport, bases)
    pub text_hex: &'static str,   // hex number
    pub text_gg: &'static str,    // gas-giant disc/ring
    pub text_uwp: &'static str,   // UWP line
    pub text_alleg: &'static str, // allegiance code

    // ── capitals / homeworlds (Worlds.xml) ──
    pub capital: &'static str,      // capital name text
    pub capital_fill: &'static str, // capital/homeworld dot (Wheat)

    // ── macro / mega region names ──
    pub macro_name: &'static str, // bold white major polity/region/rift names
    pub mega_name: &'static str,  // galaxy-scale mega labels (slightly translucent white)

    // ── sector/subsector watermark names (fadeSectorSubsectorNames cascade) ──
    pub name_full: &'static str, // foregroundColor (scale < 16)
    pub name_dark: &'static str, // DarkGray (< 48)
    pub name_dim: &'static str,  // DimGray (>=48)

    // ── star field (4 brightness tiers) ──
    pub stars: [&'static str; 4],

    // ── flags ──
    pub show_galaxy: bool, // draw the galaxy background image at macro zoom
}

impl Theme {
    /// The default ("Poster") preset — today's exact values. Reference-named colors
    /// come from the `common` consts (single source); the rest are inlined.
    pub fn poster() -> Self {
        Self {
            background: "#000000",
            macro_border: C_BORDER,
            route: C_ROUTE,
            rift: C_RIFT,
            amber: C_AMBER,
            red: C_RED,
            world_water: C_WATER,
            world_dry: C_DRY,
            vacuum_fill: "#000000",
            ag_green: "#048104",
            rich_purple: "#a000a0",
            ind_gray: "#888888",
            exotic_rust: "#cc6626",
            text: "#e9eef9",
            text_hex: "#9aa3b8",
            text_gg: "#cfd6e6",
            text_uwp: "#c9d2e4",
            text_alleg: "#aab3c8",
            capital: "#e8636f",
            capital_fill: "#f5deb3",
            macro_name: "#ffffff",
            mega_name: "rgba(255,255,255,0.92)",
            name_full: "#ffffff",
            name_dark: "#a9a9a9",
            name_dim: "#696969",
            stars: [
                "rgba(170,180,205,0.35)",
                "rgba(205,215,235,0.55)",
                "rgba(230,235,250,0.75)",
                "rgba(255,255,255,0.9)",
            ],
            show_galaxy: true,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::poster()
    }
}
