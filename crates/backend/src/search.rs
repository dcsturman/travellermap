//! In-memory name search over worlds, sectors, and subsectors.
//!
//! First pass: a flat index of lowercased names matched by the reference's
//! word-boundary `LIKE` rule and ranked by world importance (the reference
//! `SearchHandler` orders by `Importance` descending). No external service (an
//! in-process index, per the "no datastore" decision). A Tantivy full-text index
//! — tokenization, typo tolerance — is the planned upgrade; the `/api/search`
//! contract stays the same.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tmap_core::{
    astrometrics::parse_hex,
    dto::{SearchItem, SearchSector, SearchSubsector, SearchWorld, Universe},
    parse::{parse_milieu_index, sector_subsectors},
};

use crate::{milieu_metafiles, read_text, resolve_and_parse_worlds, resolve_ci, DEFAULT_MILIEU};

/// Pre-sort group order (reference `PerformSearch` emits Sectors, then
/// Subsectors, then Worlds; the final stable `OrderByDescending(Importance)`
/// keeps that order among importance ties).
const KIND_SECTOR: u8 = 0;
const KIND_SUBSECTOR: u8 = 1;
const KIND_WORLD: u8 = 2;

pub struct SearchEntry {
    name_lower: String,
    /// Group order used as a sort tiebreaker (see `KIND_*`).
    kind: u8,
    /// World importance (`{Ix}`), the reference's primary ranking key; `None`
    /// for sectors/subsectors (and worlds without an importance extension),
    /// which sort after any importance-bearing hit.
    importance: Option<i32>,
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

fn push(entries: &mut Vec<SearchEntry>, kind: u8, importance: Option<i32>, item: SearchItem) {
    entries.push(SearchEntry {
        name_lower: item_name(&item).to_lowercase(),
        kind,
        importance,
        item,
    });
}

/// Parse a raw `{Ix}` importance extension (e.g. `"{ 4 }"`) to its integer value,
/// mirroring the reference `World.ImportanceValue` (strip braces, parse; `None`
/// when absent or non-numeric).
fn importance_value(raw: Option<&str>) -> Option<i32> {
    raw?.replace(['{', '}'], " ").trim().parse().ok()
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

/// Map each milieu sector's grid position to the directory its data/metadata
/// files live in. Most sectors sit in `Sectors/{milieu}`, but cross-milieu
/// metafiles (e.g. `Faraway/faraway.xml`) place their sectors' files alongside
/// the metafile — mirrors `load_universe`'s metafile walk so search resolves
/// those files too (otherwise Faraway worlds/subsectors are silently unindexed).
fn sector_dirs(res_dir: &Path, milieu: &str) -> HashMap<(i32, i32), PathBuf> {
    let sectors_dir = res_dir.join("Sectors");
    let mut dirs: HashMap<(i32, i32), PathBuf> = HashMap::new();
    for (path, tags) in milieu_metafiles(res_dir) {
        if tags.split(',').any(|t| t.trim() == "meta") {
            continue;
        }
        let metafile_path = sectors_dir.join(&path);
        let Ok(text) = read_text(&metafile_path) else {
            continue;
        };
        let base_dir = metafile_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| sectors_dir.clone());
        for e in parse_milieu_index(&text) {
            let canonical = e.milieu.as_deref().unwrap_or(DEFAULT_MILIEU);
            if !canonical.eq_ignore_ascii_case(milieu) {
                continue;
            }
            // First metafile wins (matches `load_universe`'s `TryAdd` dedup).
            dirs.entry((e.location.x, e.location.y)).or_insert_with(|| base_dir.clone());
        }
    }
    dirs
}

/// Build the index for a milieu: every sector, its named subsectors, and every
/// named world. Each entry is already in the public [`SearchItem`] shape so the
/// handler just wraps the hits in the `{"Results":{…}}` envelope.
pub fn build_index(res_dir: &Path, milieu: &str, universe: &Universe) -> Vec<SearchEntry> {
    let mut entries = Vec::new();
    let milieu_dir = res_dir.join("Sectors").join(milieu);
    let dirs = sector_dirs(res_dir, milieu);
    for sector in &universe.sectors {
        let (sx, sy) = (sector.location.x, sector.location.y);
        let tags = sector_tags(&sector.tags, sector.metafile_tag.as_deref());
        let dir = dirs.get(&(sx, sy)).cloned().unwrap_or_else(|| milieu_dir.clone());
        push(
            &mut entries,
            KIND_SECTOR,
            None,
            SearchItem::Sector(SearchSector {
                sector_x: sx,
                sector_y: sy,
                name: sector.name.clone(),
                sector_tags: tags.clone(),
            }),
        );

        // Subsectors: named entries from the sector's metadata `.xml`, resolved
        // case-insensitively (the same `MetadataFile` else data-stem else name
        // fallback the data handlers use).
        let meta_file = sector
            .metadata_file
            .clone()
            .or_else(|| {
                sector
                    .data_file
                    .as_deref()
                    .and_then(|df| Path::new(df).file_stem().and_then(|s| s.to_str()))
                    .map(|stem| format!("{stem}.xml"))
            })
            .unwrap_or_else(|| format!("{}.xml", sector.name));
        if let Some(meta_xml) = resolve_ci(&dir, &meta_file).and_then(|p| read_text(p).ok()) {
            for sub in sector_subsectors(&meta_xml) {
                push(
                    &mut entries,
                    KIND_SUBSECTOR,
                    None,
                    SearchItem::Subsector(SearchSubsector {
                        sector: sector.name.clone(),
                        index: sub.index,
                        sector_x: sx,
                        sector_y: sy,
                        name: sub.name,
                        sector_tags: tags.clone(),
                    }),
                );
            }
        }

        // Worlds: resolve the sector's data file (handles `.tab`/`.txt`/`.sec`
        // formats and case-insensitive filenames) the same way the route + data
        // handlers do.
        let Some((_file, outcome)) = resolve_and_parse_worlds(&dir, &sector.name, Some(sector))
        else {
            continue;
        };
        for world in outcome.worlds {
            if world.name.is_empty() {
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
            let importance = importance_value(world.importance.as_deref());
            push(
                &mut entries,
                KIND_WORLD,
                importance,
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

/// Does `name` match `query` under the reference's word-boundary `LIKE` rule
/// (`name LIKE term + '%' OR name LIKE '% ' + term + '%'`): the name starts with
/// the term, or contains the term at a word boundary (preceded by a space).
fn name_matches(name_lower: &str, q: &str) -> bool {
    name_lower.starts_with(q) || name_lower.contains(&format!(" {q}"))
}

/// Top `limit` results for `query`. Matches on the word-boundary rule, then
/// orders by world importance (descending; `None` last), then group order
/// (Sector < Subsector < World), then name — the reference's stable
/// `OrderByDescending(Importance)` over the grouped result set.
pub fn search(entries: &[SearchEntry], query: &str, limit: usize) -> Vec<SearchItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<&SearchEntry> =
        entries.iter().filter(|e| name_matches(&e.name_lower, &q)).collect();
    hits.sort_by(|a, b| {
        // `None` importance sorts last: `Option` orders `None < Some`, so a
        // descending compare (`b` vs `a`) places `None` after any `Some`.
        b.importance
            .cmp(&a.importance)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name_lower.cmp(&b.name_lower))
    });
    hits.into_iter().take(limit).map(|e| e.item.clone()).collect()
}
