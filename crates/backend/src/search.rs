//! In-memory name search over worlds and sectors.
//!
//! First pass: a flat index of lowercased names scored by exact/prefix/contains
//! match. No external service (an in-process index, per the "no datastore"
//! decision). A Tantivy full-text index — ranking, tokenization, typo tolerance
//! — is the planned upgrade; the `/api/search` contract stays the same.

use std::path::Path;

use tmap_core::{
    astrometrics::{parse_hex, Coord},
    dto::{SearchResult, Universe},
    parse::parse_tab,
};

pub struct SearchEntry {
    name_lower: String,
    result: SearchResult,
}

fn push(entries: &mut Vec<SearchEntry>, result: SearchResult) {
    entries.push(SearchEntry {
        name_lower: result.name.to_lowercase(),
        result,
    });
}

/// Build the index for a milieu: every named world plus every sector.
pub fn build_index(res_dir: &Path, milieu: &str, universe: &Universe) -> Vec<SearchEntry> {
    let mut entries = Vec::new();
    for sector in &universe.sectors {
        push(
            &mut entries,
            SearchResult {
                name: sector.name.clone(),
                kind: "sector".into(),
                sector: sector.name.clone(),
                hex: None,
                coord: Coord::new(sector.location.x * 32 + 16, sector.location.y * 40 + 20),
            },
        );
        let path = res_dir
            .join("Sectors")
            .join(milieu)
            .join(format!("{}.tab", sector.name));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = parse_tab(&text) else {
            continue;
        };
        for world in parsed.worlds {
            if world.name.is_empty() {
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
            push(
                &mut entries,
                SearchResult {
                    name: world.name,
                    kind: "world".into(),
                    sector: sector.name.clone(),
                    coord: Coord::new(sector.location.x * 32 + col, sector.location.y * 40 + row),
                    hex: Some(world.hex),
                },
            );
        }
    }
    entries
}

/// Top `limit` results for `query`, ranked exact > prefix > contains, then by
/// shorter/alphabetical name.
pub fn search(entries: &[SearchEntry], query: &str, limit: usize) -> Vec<SearchResult> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i32, &SearchEntry)> = entries
        .iter()
        .filter_map(|e| {
            let score = if e.name_lower == q {
                3
            } else if e.name_lower.starts_with(&q) {
                2
            } else if e.name_lower.contains(&q) {
                1
            } else {
                0
            };
            (score > 0).then_some((score, e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.result.name.len().cmp(&b.1.result.name.len()))
            .then_with(|| a.1.result.name.cmp(&b.1.result.name))
    });
    scored.into_iter().take(limit).map(|(_, e)| e.result.clone()).collect()
}
