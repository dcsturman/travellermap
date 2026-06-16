//! In-memory name search over worlds and sectors.
//!
//! First pass: a flat index of lowercased names scored by exact/prefix/contains
//! match. No external service (an in-process index, per the "no datastore"
//! decision). A Tantivy full-text index — ranking, tokenization, typo tolerance
//! — is the planned upgrade; the `/api/search` contract stays the same.

use std::path::Path;

use tmap_core::{
    astrometrics::parse_hex,
    dto::{SearchItem, SearchSector, SearchWorld, Universe},
    parse::parse_tab,
};

pub struct SearchEntry {
    name_lower: String,
    item: SearchItem,
}

/// The displayed name of an item — the field searches match against.
fn item_name(item: &SearchItem) -> &str {
    match item {
        SearchItem::World(w) => &w.name,
        SearchItem::Sector(s) => &s.name,
        SearchItem::Subsector(s) => &s.name,
    }
}

fn push(entries: &mut Vec<SearchEntry>, item: SearchItem) {
    entries.push(SearchEntry {
        name_lower: item_name(&item).to_lowercase(),
        item,
    });
}

/// The sector's `SectorTags` — its own review tags plus the milieu metafile tag,
/// deduped preserving order (matches the `/api/universe` `Tags` builder).
fn sector_tags(tags: &str, metafile_tag: Option<&str>) -> String {
    let mut seen = std::collections::HashSet::new();
    [tags, metafile_tag.unwrap_or("")]
        .into_iter()
        .flat_map(str::split_whitespace)
        .filter(|t| seen.insert(*t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the index for a milieu: every named world plus every sector. Each entry
/// is already in the public [`SearchItem`] shape so the handler just wraps the
/// hits in the `{"Results":{…}}` envelope.
pub fn build_index(res_dir: &Path, milieu: &str, universe: &Universe) -> Vec<SearchEntry> {
    let mut entries = Vec::new();
    for sector in &universe.sectors {
        let (sx, sy) = (sector.location.x, sector.location.y);
        let tags = sector_tags(&sector.tags, sector.metafile_tag.as_deref());
        push(
            &mut entries,
            SearchItem::Sector(SearchSector {
                sector_x: sx,
                sector_y: sy,
                name: sector.name.clone(),
                sector_tags: tags.clone(),
            }),
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
                SearchItem::World(SearchWorld {
                    hex_x: col,
                    hex_y: row,
                    sector: sector.name.clone(),
                    uwp: world.uwp,
                    sector_x: sx,
                    sector_y: sy,
                    name: world.name,
                    sector_tags: tags.clone(),
                }),
            );
        }
    }
    entries
}

/// Top `limit` results for `query`, ranked exact > prefix > contains, then by
/// shorter/alphabetical name.
pub fn search(entries: &[SearchEntry], query: &str, limit: usize) -> Vec<SearchItem> {
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
        let (na, nb) = (item_name(&a.1.item), item_name(&b.1.item));
        b.0.cmp(&a.0)
            .then_with(|| na.len().cmp(&nb.len()))
            .then_with(|| na.cmp(nb))
    });
    scored.into_iter().take(limit).map(|(_, e)| e.item.clone()).collect()
}
