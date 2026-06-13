//! Map "status" overlays: (A) a translucent dim wash over sectors that aren't
//! official/reviewed, and (B) a bottom footer crediting the data source of the
//! sector under focus plus the Traveller trademark line.
//!
//! These mirror the reference site's treatment of unofficial sectors (drawn
//! dimmed) and its per-sector `<Credits>` attribution. They're screen-space
//! passes drawn after the world layers. NOT wired into `render/mod.rs` here —
//! the orchestrator adds them to the frame and removes the `#[allow(dead_code)]`.

use crate::canvas::{Canvas, TextAlign};
use super::common::{hex_parsec, on_screen, ViewState, SECTOR_H, SECTOR_W};
use tmap_core::dto::SectorData;

/// Tags that mark a sector as "official enough" to draw at full brightness.
const OFFICIAL_TAGS: [&str; 3] = ["Official", "Preserve", "InReview"];

/// Dim every sector that isn't tagged official/reviewed (empty tags count as
/// unofficial) with a translucent dark quad over its bounding box.
#[allow(dead_code)]
pub(crate) fn draw_dim_overlay(c: &impl Canvas, view: &ViewState, w: f64, h: f64, sectors: &[&SectorData]) {
    for sd in sectors {
        let Some(loc) = sd.info.location else { continue };
        if OFFICIAL_TAGS.iter().any(|t| sd.info.tags.contains(t)) {
            continue; // official / reviewed → full brightness
        }
        // Four corner world hexes of the sector (cols 1..=W, rows 1..=H).
        let (c0, c1) = (loc.x * SECTOR_W + 1, loc.x * SECTOR_W + SECTOR_W);
        let (r0, r1) = (loc.y * SECTOR_H + 1, loc.y * SECTOR_H + SECTOR_H);
        let corners = [(c0, r0), (c1, r0), (c1, r1), (c0, r1)];
        let quad: Vec<(f64, f64)> = corners
            .iter()
            .map(|&(wc, wr)| view.to_screen(w, h, hex_parsec(wc, wr)))
            .collect();
        // Skip if the whole quad is off-screen.
        if quad.iter().all(|&(x, y)| !on_screen(x, y, w, h, 0.0)) {
            continue;
        }
        c.fill_polygons(&[quad], "#000000", 0.45);
    }
}

/// Draw a full-width bottom footer: the focused sector's data-source credit on
/// the left, the Traveller trademark + tagline on the right.
#[allow(dead_code)]
pub(crate) fn draw_footer(c: &impl Canvas, w: f64, h: f64, sector: &SectorData) {
    // Backing bar.
    c.fill_polygons(
        &[vec![(0.0, h - 26.0), (w, h - 26.0), (w, h), (0.0, h)]],
        "#05070d",
        0.82,
    );

    // LEFT: sector name + data-source credit (plain text, truncated).
    let name = &sector.info.name;
    let left = match &sector.info.credits {
        Some(raw) => {
            let credit = plain_credit(raw);
            if credit.is_empty() {
                name.clone()
            } else {
                format!("{name} — {credit}")
            }
        }
        None => name.clone(),
    };
    c.fill_text(
        &left,
        12.0,
        h - 13.0,
        "rgba(200,206,228,0.85)",
        "12px Arial, sans-serif",
        TextAlign::Left,
    );

    // RIGHT: Traveller trademark + tagline.
    c.fill_text(
        "TRAVELLER®",
        w - 12.0,
        h - 15.0,
        "rgba(227,39,54,0.95)",
        "700 13px Georgia, 'Times New Roman', serif",
        TextAlign::Right,
    );
    c.fill_text(
        "Science Fiction Adventure in the Far Future",
        w - 12.0,
        h - 4.0,
        "rgba(180,150,150,0.8)",
        "italic 9px Georgia, serif",
        TextAlign::Right,
    );
}

/// Turn raw `<Credits>` text (possibly HTML-encoded with markup) into a short
/// plain-text line: unescape entities, strip tags, collapse whitespace, and
/// truncate to ~160 chars with an ellipsis.
fn plain_credit(s: &str) -> String {
    // HTML-unescape the common entities.
    let unescaped = s
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");

    // Strip `<...>` tags (drop everything between `<` and the next `>`).
    let mut stripped = String::with_capacity(unescaped.len());
    let mut in_tag = false;
    for ch in unescaped.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(ch),
            _ => {}
        }
    }

    // Collapse runs of whitespace to single spaces.
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");

    // Truncate to ~160 chars on a char boundary, appending an ellipsis.
    const MAX: usize = 160;
    if collapsed.chars().count() > MAX {
        let mut out: String = collapsed.chars().take(MAX).collect();
        out.push('…');
        out
    } else {
        collapsed
    }
}
