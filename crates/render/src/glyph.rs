//! Base-code glyph table, ported from the reference `RenderUtil` Glyph table
//! (`s_baseGlyphTable` / `Glyph.FromBaseCode`). Maps a world's base codes to
//! the classic Traveller symbols (scout triangle, naval star, depot square, …).

/// Which base slot a glyph prefers — top-left or bottom-left of the world.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    Top,
    Bottom,
    None,
}

pub struct BaseGlyph {
    pub chars: &'static str,
    pub bias: Bias,
    /// Drawn in the highlight color (red) rather than the normal text color.
    pub highlight: bool,
}

// Symbols (Unicode), matching the reference Glyph constants.
const SQUARE: &str = "■"; // U+25A0  Depot
const STAR5: &str = "★"; // U+2605  Naval base
const STAR4: &str = "✦"; // U+2726  Military base
const STARSTAR: &str = "✶✶"; // reference "**" (corsair/embassy/clan)
const TRIANGLE: &str = "▲"; // U+25B2  Scout base / waystation
const CIRCLE: &str = "●"; // U+25CF  Exploration base
const DIAMOND: &str = "◆"; // U+2666  Zhodani relay

/// Glyph for a base `code`, given the world's (base) allegiance. Mirrors the
/// regex table: specific-allegiance patterns (`Im.D`, `Zh.W`) win over the
/// wildcard `*.code`.
pub fn base_glyph(allegiance: &str, code: char) -> Option<BaseGlyph> {
    let a = allegiance.get(..2).unwrap_or(allegiance);
    let g = |chars, bias, highlight| {
        Some(BaseGlyph {
            chars,
            bias,
            highlight,
        })
    };
    match (a, code) {
        ("Im", 'D') => g(SQUARE, Bias::Bottom, false), // Imperial Depot
        ("Zh", 'W') => g(DIAMOND, Bias::None, true),   // Zhodani Relay Station
        (_, 'C') => g(STARSTAR, Bias::Bottom, false),  // Vargr Corsair Base
        (_, 'D') => g(SQUARE, Bias::None, true),       // Depot
        (_, 'E') => g(STARSTAR, Bias::Bottom, false),  // Hiver Embassy
        (_, 'K') => g(STAR5, Bias::Top, true),         // Naval Base
        (_, 'M') => g(STAR4, Bias::Bottom, false),     // Military Base
        (_, 'N') => g(STAR5, Bias::Top, false),        // Imperial Naval Base
        (_, 'O') => g(SQUARE, Bias::Top, true),        // K'kree Naval Outpost
        (_, 'R') => g(STARSTAR, Bias::Bottom, false),  // Aslan Clan Base
        (_, 'S') => g(TRIANGLE, Bias::Bottom, false),  // Imperial Scout Base
        (_, 'T') => g(STAR5, Bias::Top, true),         // Aslan Tlaukhu Base
        (_, 'V') => g(CIRCLE, Bias::Bottom, false),    // Exploration Base
        (_, 'W') => g(TRIANGLE, Bias::Bottom, true),   // Imperial Scout Waystation
        _ => None,
    }
}
