//! In-memory search over worlds, sectors, subsectors, and labeled regions.
//!
//! A flat index of [`SearchEntry`]s built from `res/`, queried with the full
//! travellermap.com query language (ported in `tmap_core::searchlang`: the
//! `LIKE`/`SOUNDEX` matcher + the per-term clause table from
//! `server/search/SearchEngine.cs`). No external service — an in-process index,
//! per the "no datastore" decision. (Tantivy would be a later upgrade; the
//! `/api/search` contract stays the same.)
//!
//! The reference pushes clauses to SQL Server; here every entry carries the same
//! columns the `worlds`/`sectors`/`subsectors`/`labels` tables held, and the
//! clauses run in Rust over them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tmap_core::{
    astrometrics::{coordinates_to_location, location_to_coordinates, parse_hex},
    dto::{SearchItem, SearchLabel, SearchSector, SearchSubsector, SearchWorld, Universe},
    metadata::parse_sector_metadata,
    parse::{parse_milieu_index, sector_subsectors},
    searchlang::{ParsedQuery, SearchRecord, SearchTypes},
};

use crate::{milieu_metafiles, read_text, resolve_and_parse_worlds, resolve_ci, DEFAULT_MILIEU};

/// Pre-sort group order (reference `PerformSearch` emits Sectors, Subsectors,
/// Worlds, then Labels; the final stable `OrderByDescending(Importance)` keeps
/// that order among importance ties).
const KIND_SECTOR: u8 = 0;
const KIND_SUBSECTOR: u8 = 1;
const KIND_WORLD: u8 = 2;
const KIND_LABEL: u8 = 3;

/// The per-world columns the field ops (`uwp:`, `stellar:`, …) match against —
/// lowercased, mirroring the reference `worlds` table. Empty for non-world
/// entries.
#[derive(Default)]
struct WorldFields {
    uwp: String,
    pbg: String,
    zone: String,
    alleg: String,
    stellar: String,
    remarks: String,
    ex: String,
    cx: String,
    ix: Option<i32>,
    sector_name: String,
}

pub struct SearchEntry {
    name_lower: String,
    /// Group order used as a sort tiebreaker (see `KIND_*`).
    kind: u8,
    /// World importance (`{Ix}`), the reference's primary ranking key; `None`
    /// for sectors/subsectors/labels (and worlds without an importance
    /// extension), which sort after any importance-bearing hit.
    importance: Option<i32>,
    fields: WorldFields,
    item: SearchItem,
}

impl SearchEntry {
    fn record(&self) -> SearchRecord<'_> {
        SearchRecord {
            name: &self.name_lower,
            uwp: &self.fields.uwp,
            pbg: &self.fields.pbg,
            zone: &self.fields.zone,
            alleg: &self.fields.alleg,
            stellar: &self.fields.stellar,
            remarks: &self.fields.remarks,
            ex: &self.fields.ex,
            cx: &self.fields.cx,
            ix: self.fields.ix,
            sector_name: &self.fields.sector_name,
        }
    }

    /// Does this entry's kind fall within the requested type set?
    fn kind_allowed(&self, types: &SearchTypes) -> bool {
        match self.kind {
            KIND_SECTOR => types.sectors,
            KIND_SUBSECTOR => types.subsectors,
            KIND_WORLD => types.worlds,
            KIND_LABEL => types.labels,
            _ => false,
        }
    }
}

/// The displayed name of an item — the field searches match against.
fn item_name(item: &SearchItem) -> &str {
    match item {
        SearchItem::World(w) => &w.name,
        SearchItem::Sector(s) => &s.name,
        SearchItem::Subsector(s) => &s.name,
        SearchItem::Label(l) => &l.name,
    }
}

fn push(entries: &mut Vec<SearchEntry>, kind: u8, importance: Option<i32>, fields: WorldFields, item: SearchItem) {
    entries.push(SearchEntry {
        name_lower: item_name(&item).to_lowercase(),
        kind,
        importance,
        fields,
        item,
    });
}

/// Parse a raw `{Ix}` importance extension (e.g. `"{ 4 }"`) to its integer
/// value, mirroring the reference `World.ImportanceValue` (strip braces, parse;
/// `None` when absent or non-numeric).
fn importance_value(raw: Option<&str>) -> Option<i32> {
    raw?.replace(['{', '}'], " ").trim().parse().ok()
}

/// Strip every kind of bracket from an extension's raw form (reference
/// `StripBrackets`): `"(C53-1)"` → `"c53-1"`, `"[6559]"` → `"6559"`. Lowercased
/// for case-insensitive `LIKE`.
fn strip_brackets(raw: Option<&str>) -> String {
    raw.unwrap_or("").replace(['(', ')', '[', ']', '{', '}'], "").to_lowercase()
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

/// Whether a sector is indexed: the reference only indexes OTU/Faraway-tagged
/// sectors (`SearchEngine.PopulateDatabase`). The combined tag string already
/// folds in the metafile tag (e.g. `Faraway`).
fn is_searchable(tags: &str) -> bool {
    tags.split_whitespace().any(|t| t == "OTU" || t == "Faraway")
}

/// Accumulated label points keyed by (label text) — averaged into one hit at the
/// end, with a radius bucket from the spread (reference `PopulateDatabase`'s
/// label aggregation). Each milieu builds its own index, so the milieu key is
/// implicit.
#[derive(Default)]
struct LabelAccum {
    points: Vec<(i32, i32)>,
}

/// Collapse runs of whitespace and trim (reference `SanifyLabel`).
fn sanify_label(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build the index for a milieu: every searchable sector, its named subsectors,
/// every named world (with its searchable fields), and the aggregated labeled
/// regions (border labels + `<Label>` elements). Each entry is already in the
/// public [`SearchItem`] shape so the handler just wraps the hits in the
/// `{"Results":{…}}` envelope.
pub fn build_index(res_dir: &Path, milieu: &str, universe: &Universe) -> Vec<SearchEntry> {
    let mut entries = Vec::new();
    let milieu_dir = res_dir.join("Sectors").join(milieu);
    let dirs = sector_dirs(res_dir, milieu);

    // Label aggregation: (sanified text) -> points. Resolved to hits after the
    // sector loop so multi-sector regions average correctly.
    let mut labels: HashMap<String, LabelAccum> = HashMap::new();

    for sector in &universe.sectors {
        let (sx, sy) = (sector.location.x, sector.location.y);
        let tags = sector_tags(&sector.tags, sector.metafile_tag.as_deref());
        if !is_searchable(&tags) {
            continue;
        }
        let dir = dirs.get(&(sx, sy)).cloned().unwrap_or_else(|| milieu_dir.clone());

        push(
            &mut entries,
            KIND_SECTOR,
            None,
            WorldFields::default(),
            SearchItem::Sector(SearchSector {
                sector_x: sx,
                sector_y: sy,
                name: sector.name.clone(),
                sector_tags: tags.clone(),
            }),
        );

        // Metadata `.xml`: named subsectors + border/standalone labels. Resolved
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
                    WorldFields::default(),
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

            // Labels: border labels (where shown) + hand-placed `<Label>`s.
            let meta = parse_sector_metadata(&meta_xml);
            for b in meta.borders.iter().chain(meta.regions.iter()) {
                if !b.show_label {
                    continue;
                }
                // Reference `Border.GetLabel`: an explicit label, else the
                // border's allegiance resolved to its name (the sector's own
                // `<Allegiance>` table first, then the stock T5SS codes — mirrors
                // `Sector.GetAllegianceFromCode`). Borders with neither contribute
                // no label point. Getting this right matters: a region's search
                // position is the *average* of all its label points, so a missing
                // allegiance-derived point skews the reported hex.
                let text = match b.label.as_deref().filter(|s| !s.is_empty()) {
                    Some(l) => l.to_string(),
                    None => match b.allegiance.as_deref().and_then(|c| resolve_alleg_name(&meta, c)) {
                        Some(n) => n,
                        None => continue,
                    },
                };
                let pos = label_position_of(b);
                if let Some((col, row)) = parse_hex(&pos) {
                    let (cx, cy) = location_to_coordinates(sx, sy, col, row);
                    labels.entry(sanify_label(&text)).or_default().points.push((cx, cy));
                }
            }
            for l in &meta.labels {
                if l.text.is_empty() {
                    continue;
                }
                if let Some((col, row)) = parse_hex(&l.hex) {
                    let (cx, cy) = location_to_coordinates(sx, sy, col, row);
                    labels.entry(sanify_label(&l.text)).or_default().points.push((cx, cy));
                }
            }
        }

        // Worlds: resolve the sector's data file (handles `.tab`/`.txt`/`.sec`
        // formats and case-insensitive filenames) the same way the route + data
        // handlers do.
        let Some((file, mut outcome)) = resolve_and_parse_worlds(&dir, &sector.name, Some(sector))
        else {
            continue;
        };
        // Some `.txt` data files are actually the legacy fixed-column SEC format
        // (a `#----` ruler comment header) rather than the T5 dash-ruler column
        // format the extension implies — `parse_column` yields nothing for them
        // (e.g. Faraway/Virgo.txt). Fall back to the SEC parser so those worlds
        // (Reginante, …) are indexed, matching the live reference.
        if outcome.worlds.is_empty() {
            if let Some(text) = resolve_ci(&dir, &file).and_then(|p| read_text(p).ok()) {
                if let Ok(sec) = tmap_core::parse::parse_sec(&text) {
                    if !sec.worlds.is_empty() {
                        outcome = sec;
                    }
                }
            }
        }
        for world in outcome.worlds {
            if world.name.is_empty() {
                continue;
            }
            let Some((col, row)) = parse_hex(&world.hex) else {
                continue;
            };
            let importance = importance_value(world.importance.as_deref());
            let fields = WorldFields {
                uwp: world.uwp.to_lowercase(),
                pbg: world.pbg.to_lowercase(),
                // Reference stores "G" when the zone is empty.
                zone: if world.zone.is_empty() { "g".to_string() } else { world.zone.to_lowercase() },
                alleg: world.allegiance.to_lowercase(),
                stellar: world.stellar.to_lowercase(),
                remarks: world.remarks.to_lowercase(),
                ex: strip_brackets(world.economic.as_deref()),
                cx: strip_brackets(world.cultural.as_deref()),
                ix: importance,
                sector_name: sector.name.to_lowercase(),
            };
            push(
                &mut entries,
                KIND_WORLD,
                importance,
                fields,
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

    // Resolve aggregated labels into hits. The sector tags come from re-resolving
    // the sector at the averaged coordinate (mirrors the reference, which
    // resolves the label's sector from its `Coords`).
    let tag_by_loc: HashMap<(i32, i32), String> = universe
        .sectors
        .iter()
        .map(|s| {
            (
                (s.location.x, s.location.y),
                sector_tags(&s.tags, s.metafile_tag.as_deref()),
            )
        })
        .collect();
    for (name, accum) in labels {
        if accum.points.is_empty() {
            continue;
        }
        let n = accum.points.len() as i64;
        let avg_x = (accum.points.iter().map(|p| p.0 as i64).sum::<i64>() as f64 / n as f64).round() as i32;
        let avg_y = (accum.points.iter().map(|p| p.1 as i64).sum::<i64>() as f64 / n as f64).round() as i32;
        let (min_x, max_x) = accum.points.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (min_y, max_y) = accum.points.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        let radius = (max_x - min_x).max(max_y - min_y);
        let scale = if radius > 80 {
            4
        } else if radius > 40 {
            8
        } else if radius > 20 {
            32
        } else {
            64
        };
        let (lsx, lsy, lhx, lhy) = coordinates_to_location(avg_x, avg_y);
        let label_tags = tag_by_loc.get(&(lsx, lsy)).cloned().unwrap_or_default();
        push(
            &mut entries,
            KIND_LABEL,
            None,
            WorldFields::default(),
            SearchItem::Label(SearchLabel {
                hex_x: lhx,
                hex_y: lhy,
                scale,
                sector_x: lsx,
                sector_y: lsy,
                name,
                sector_tags: label_tags,
            }),
        );
    }

    entries
}

/// Reference `Sector.GetAllegianceFromCode`: resolve a T5 allegiance code to a
/// name via the sector's own `<Allegiance>` table first, then the stock T5SS
/// codes. Used to label a border by its allegiance when it has no explicit text.
fn resolve_alleg_name(meta: &tmap_core::metadata::SectorMetadata, code: &str) -> Option<String> {
    // `parse_sector_metadata` puts the sector's `<Allegiance>` elements in
    // `local_allegiances` (the reference's `Sector.Allegiances`); fall back to
    // the stock T5SS codes, matching `Sector.GetAllegianceFromCode`.
    meta.local_allegiances
        .iter()
        .find(|a| a.code == code)
        .map(|a| a.name.clone())
        .or_else(|| tmap_core::world_util::allegiance_name(code))
}

/// A border's label position: the explicit `LabelPosition`, else the
/// bounding-box centre of its hex path (reference `Border.LabelPosition`).
fn label_position_of(b: &tmap_core::metadata::MetaBorder) -> String {
    if let Some(p) = &b.label_position {
        return p.clone();
    }
    let coords: Vec<(i32, i32)> = b.hexes.iter().filter_map(|h| parse_hex(h)).collect();
    if coords.is_empty() {
        return "0000".to_string();
    }
    let (min_x, max_x) = coords.iter().fold((i32::MAX, i32::MIN), |(a, b), &(x, _)| (a.min(x), b.max(x)));
    let (min_y, max_y) = coords.iter().fold((i32::MAX, i32::MIN), |(a, b), &(_, y)| (a.min(y), b.max(y)));
    format!("{:02}{:02}", (min_x + max_x + 1) / 2, (min_y + max_y + 1) / 2)
}

/// Run a parsed query over the index and return up to `limit` hits.
///
/// Mirrors the reference `PerformSearch` + handler ordering: each result kind is
/// filtered, then **ranked by importance descending** (matching the live
/// reference, whose handler sorts the whole result set by `Importance` before
/// `Take(NUM_RESULTS)`) and capped to `limit` per kind; the kinds are
/// concatenated in (Sector, Subsector, World, Label) order; finally the merged
/// set is stably re-sorted by importance descending and capped to `limit` again.
/// The per-kind importance ranking (rather than the reference subquery's name
/// ordering) is what keeps high-importance worlds like Regina in the top results
/// for broad queries such as `r*a`.
pub fn run_query(entries: &[SearchEntry], pq: &ParsedQuery, limit: usize) -> Vec<SearchItem> {
    // The reference returns nothing when the parse produced no clauses (and it's
    // not the sector+hex shortcut).
    if pq.clauses.is_empty() && pq.sector_hex.is_none() {
        return Vec::new();
    }

    let matches = |e: &SearchEntry| -> bool {
        if !e.kind_allowed(&pq.types) {
            return false;
        }
        if let Some(sh) = &pq.sector_hex {
            // Only worlds participate; sector_name starts-with prefix AND hex.
            if e.kind != KIND_WORLD {
                return false;
            }
            if let SearchItem::World(w) = &e.item {
                return w.hex_x == sh.hex_x
                    && w.hex_y == sh.hex_y
                    && e.fields.sector_name.starts_with(&sh.sector_prefix);
            }
            return false;
        }
        let rec = e.record();
        pq.clauses.iter().all(|c| c.matches(&rec))
    };

    // Group matches by kind, rank each kind by importance descending (stable, so
    // index order — i.e. name order within a sector's data — breaks ties), then
    // cap each kind to `limit` (reference `TOP {limit}` per table). Ranking
    // before the cap keeps high-importance hits from being truncated away.
    let mut by_kind: [Vec<&SearchEntry>; 4] = Default::default();
    for e in entries.iter().filter(|e| matches(e)) {
        let bucket = e.kind as usize;
        if bucket < 4 {
            by_kind[bucket].push(e);
        }
    }
    for bucket in by_kind.iter_mut() {
        bucket.sort_by_key(|e| std::cmp::Reverse(e.importance));
        bucket.truncate(limit);
    }

    let mut hits: Vec<&SearchEntry> = by_kind.into_iter().flatten().collect();
    // Stable sort by importance descending; `None` sorts last (`Reverse(None)`
    // is greater than `Reverse(Some)` since `None < Some`). Kind/index order is
    // preserved among ties because the sort is stable and `hits` is already in
    // (kind, index) order.
    hits.sort_by_key(|e| std::cmp::Reverse(e.importance));
    hits.into_iter().take(limit).map(|e| e.item.clone()).collect()
}
