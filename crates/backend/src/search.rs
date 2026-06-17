//! Embedded full-text search over worlds, sectors, subsectors, and labeled
//! regions — an in-RAM **Tantivy** index built from `res/` per milieu, replacing
//! the reference's SQL Server search index (and our own earlier linear scan).
//! No external service, per the "no datastore" decision; the `/api/search`
//! contract is unchanged.
//!
//! The query language is ported in `tmap_core::searchlang` (the `LIKE`/`SOUNDEX`
//! matcher + the per-term clause table from `server/search/SearchEngine.cs`).
//! Because those semantics are SQL-`LIKE`/`SOUNDEX` rather than tokenized
//! full-text, each [`Clause`] maps onto a `RegexQuery` (LIKE → regex via
//! [`like_to_regex`]) or an exact `TermQuery` over **raw** (untokenized,
//! lowercased) fields — *not* BM25-scored tokens — so results are byte-identical
//! to the scan. Tantivy produces the matching doc set; `{Ix}`-importance ranking
//! stays in Rust ([`run_query`]). The reference's SQL `worlds`/`sectors`/
//! `subsectors`/`labels` columns become this index's fields.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tantivy::collector::DocSetCollector;
use tantivy::query::{BooleanQuery, Occur, Query, RegexQuery, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Schema, Value, INDEXED, STORED, STRING};
use tantivy::{Index, IndexReader, TantivyDocument, Term};

use tmap_core::{
    astrometrics::{coordinates_to_location, location_to_coordinates, parse_hex},
    dto::{SearchItem, SearchLabel, SearchSector, SearchSubsector, SearchWorld, Universe},
    metadata::parse_sector_metadata,
    parse::{parse_milieu_index, sector_subsectors},
    searchlang::{like_to_regex, soundex, Clause, ParsedQuery},
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
fn gather_entries(res_dir: &Path, milieu: &str, universe: &Universe) -> Vec<SearchEntry> {
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

// ---------------------------------------------------------------------------
// Tantivy index
//
// Replaces the SQL Server search index of the reference (and our own earlier
// in-memory linear scan) with an embedded, in-RAM Tantivy index built at
// startup from `res/`. Our query language is SQL-`LIKE`/`SOUNDEX`, not tokenized
// full-text, so every clause maps onto a `RegexQuery` (LIKE → regex via
// `like_to_regex`) or an exact `TermQuery` over **raw** (untokenized, lowercased)
// fields — *not* BM25-scored tokens. Ranking stays in Rust, identical to before:
// Tantivy just produces the matching doc set, then we rank by `{Ix}` importance.
// ---------------------------------------------------------------------------

/// The schema fields, kept so queries can build `Term`s / `RegexQuery`s.
struct SchemaFields {
    name: Field,
    name_soundex: Field,
    uwp: Field,
    pbg: Field,
    zone: Field,
    alleg: Field,
    ex: Field,
    cx: Field,
    sector_name: Field,
    stellar: Field,
    remarks: Field,
    ix: Field,
    hex_x: Field,
    hex_y: Field,
    kind: Field,
    blob: Field,
}

/// An embedded full-text index for one milieu: the Tantivy reader + the field
/// handles needed to build queries against it.
pub struct SearchIndex {
    reader: IndexReader,
    f: SchemaFields,
}

/// The per-hit ranking + payload, stored as one JSON `blob` field per document so
/// retrieval reconstructs the public `SearchItem` and the reference ranking keys
/// (`kind`, `{Ix}` importance, original insertion order for stable tie-breaks)
/// without a parallel side table.
#[derive(Serialize, Deserialize)]
struct Blob {
    kind: u8,
    importance: Option<i32>,
    ord: u32,
    item: SearchItem,
}

/// Build the in-RAM Tantivy index for `milieu` from `res/`. Gathers the same
/// entries the linear scan used (worlds + their fields, sectors, subsectors,
/// aggregated labels), then writes one document per entry.
pub fn build_index(res_dir: &Path, milieu: &str, universe: &Universe) -> SearchIndex {
    let entries = gather_entries(res_dir, milieu, universe);

    let mut sb = Schema::builder();
    // Raw (untokenized) lowercased text fields — `STRING` uses the `raw`
    // tokenizer, so each value is a single term and `RegexQuery` matches the
    // whole term, exactly mirroring full-string `like_match`.
    let f = SchemaFields {
        name: sb.add_text_field("name", STRING),
        name_soundex: sb.add_text_field("name_soundex", STRING),
        uwp: sb.add_text_field("uwp", STRING),
        pbg: sb.add_text_field("pbg", STRING),
        zone: sb.add_text_field("zone", STRING),
        alleg: sb.add_text_field("alleg", STRING),
        ex: sb.add_text_field("ex", STRING),
        cx: sb.add_text_field("cx", STRING),
        sector_name: sb.add_text_field("sector_name", STRING),
        // Multi-valued: one term per whitespace token, so a `RegexQuery` matches
        // a whole stellar/remark token (the `token_like` semantics).
        stellar: sb.add_text_field("stellar", STRING),
        remarks: sb.add_text_field("remarks", STRING),
        ix: sb.add_i64_field("ix", INDEXED),
        hex_x: sb.add_i64_field("hex_x", INDEXED),
        hex_y: sb.add_i64_field("hex_y", INDEXED),
        kind: sb.add_u64_field("kind", INDEXED),
        blob: sb.add_text_field("blob", STORED),
    };
    let schema = sb.build();

    let index = Index::create_in_ram(schema);
    let mut writer = index.writer(15_000_000).expect("tantivy index writer");
    for (ord, e) in entries.iter().enumerate() {
        let mut d = TantivyDocument::default();
        d.add_text(f.name, &e.name_lower);
        d.add_text(f.name_soundex, soundex(&e.name_lower));
        d.add_u64(f.kind, e.kind as u64);
        if let Some(ix) = e.importance {
            d.add_i64(f.ix, ix as i64);
        }
        let wf = &e.fields;
        for (field, val) in [
            (f.uwp, &wf.uwp),
            (f.pbg, &wf.pbg),
            (f.zone, &wf.zone),
            (f.alleg, &wf.alleg),
            (f.ex, &wf.ex),
            (f.cx, &wf.cx),
            (f.sector_name, &wf.sector_name),
        ] {
            if !val.is_empty() {
                d.add_text(field, val);
            }
        }
        for tok in wf.stellar.split_whitespace() {
            d.add_text(f.stellar, tok);
        }
        for tok in wf.remarks.split_whitespace() {
            d.add_text(f.remarks, tok);
        }
        if let SearchItem::World(w) = &e.item {
            d.add_i64(f.hex_x, w.hex_x as i64);
            d.add_i64(f.hex_y, w.hex_y as i64);
        }
        let blob = Blob {
            kind: e.kind,
            importance: e.importance,
            ord: ord as u32,
            item: e.item.clone(),
        };
        d.add_text(f.blob, serde_json::to_string(&blob).expect("serialize search blob"));
        writer.add_document(d).expect("add search document");
    }
    writer.commit().expect("commit search index");
    let reader = index.reader().expect("search index reader");
    SearchIndex { reader, f }
}

/// Run a parsed query over the index and return up to `limit` hits.
///
/// Tantivy produces the matching document set (clauses → `RegexQuery`/`TermQuery`
/// joined with `AND`); the ranking then mirrors the reference `PerformSearch` +
/// handler ordering exactly: rank each kind by `{Ix}` importance descending and
/// cap to `limit` per kind, concatenate in (Sector, Subsector, World, Label)
/// order, then stably re-sort the merged set by importance descending and cap to
/// `limit`. The per-kind importance ranking is what keeps high-importance worlds
/// like Regina in the top results for broad queries such as `r*a`.
pub fn run_query(idx: &SearchIndex, pq: &ParsedQuery, limit: usize) -> Vec<SearchItem> {
    // The reference returns nothing when the parse produced no clauses (and it's
    // not the sector+hex shortcut).
    if pq.clauses.is_empty() && pq.sector_hex.is_none() {
        return Vec::new();
    }

    let query = build_query(&idx.f, pq);
    let searcher = idx.reader.searcher();
    let Ok(docs) = searcher.search(&query, &DocSetCollector) else {
        return Vec::new();
    };

    // Reconstruct (kind, importance, ord, item) from each hit's stored blob.
    let hits = docs.into_iter().filter_map(|addr| {
        let doc: TantivyDocument = searcher.doc(addr).ok()?;
        let raw = doc.get_first(idx.f.blob)?.as_str()?;
        serde_json::from_str::<Blob>(raw).ok()
    });

    // Per-kind importance-desc + cap (ties keep insertion order via `ord`), then
    // concat in kind order, then stable global importance-desc, then cap. `None`
    // importance sorts last (Option: `None < Some`).
    let mut by_kind: [Vec<Blob>; 4] = Default::default();
    for h in hits {
        let bucket = h.kind as usize;
        if bucket < 4 {
            by_kind[bucket].push(h);
        }
    }
    for bucket in by_kind.iter_mut() {
        bucket.sort_by_key(|h| (std::cmp::Reverse(h.importance), h.ord));
        bucket.truncate(limit);
    }
    let mut merged: Vec<Blob> = by_kind.into_iter().flatten().collect();
    // Stable sort: ties keep concat order = (kind asc, ord asc).
    merged.sort_by_key(|h| std::cmp::Reverse(h.importance));
    merged.into_iter().take(limit).map(|h| h.item).collect()
}

/// Build the Tantivy query for a parsed query: a kind filter (from `types`)
/// AND-joined with either the sector+hex shortcut or every clause.
fn build_query(f: &SchemaFields, pq: &ParsedQuery) -> Box<dyn Query> {
    let mut musts: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, kind_filter(f, pq))];

    if let Some(sh) = &pq.sector_hex {
        // Worlds only, exact local hex, sector_name starts-with the prefix.
        musts.push((Occur::Must, term_u64(f.kind, KIND_WORLD as u64)));
        musts.push((Occur::Must, term_i64(f.hex_x, sh.hex_x as i64)));
        musts.push((Occur::Must, term_i64(f.hex_y, sh.hex_y as i64)));
        musts.push((Occur::Must, regex_query(f.sector_name, &format!("{}.*", like_to_regex(&sh.sector_prefix)))));
    } else {
        for c in &pq.clauses {
            musts.push((Occur::Must, clause_query(f, c)));
        }
    }
    Box::new(BooleanQuery::new(musts))
}

/// A `SHOULD` set over the allowed kinds (the `types=` filter); no allowed kind
/// → a query that matches nothing.
fn kind_filter(f: &SchemaFields, pq: &ParsedQuery) -> Box<dyn Query> {
    let t = &pq.types;
    let shoulds: Vec<(Occur, Box<dyn Query>)> = [
        (t.sectors, KIND_SECTOR),
        (t.subsectors, KIND_SUBSECTOR),
        (t.worlds, KIND_WORLD),
        (t.labels, KIND_LABEL),
    ]
    .into_iter()
    .filter(|(allowed, _)| *allowed)
    .map(|(_, k)| (Occur::Should, term_u64(f.kind, k as u64)))
    .collect();
    if shoulds.is_empty() {
        return never_match();
    }
    Box::new(BooleanQuery::new(shoulds))
}

/// Translate one parsed [`Clause`] into a Tantivy query, mirroring
/// [`Clause::matches`] term-for-term (LIKE → `RegexQuery`, SOUNDEX/Ix → exact).
fn clause_query(f: &SchemaFields, c: &Clause) -> Box<dyn Query> {
    match c {
        // name LIKE term+'%'  OR  name LIKE '% '+term+'%'  (term is a literal).
        Clause::NameWordBoundary(t) => {
            let e = like_to_regex(t);
            Box::new(BooleanQuery::new(vec![
                (Occur::Should, regex_query(f.name, &format!("{e}.*"))),
                (Occur::Should, regex_query(f.name, &format!(".* {e}.*"))),
            ]))
        }
        Clause::NameLike(t) => regex_query(f.name, &like_to_regex(t)),
        Clause::NameSoundex(t) => term_text(f.name_soundex, &soundex(t)),
        Clause::Uwp(t) => regex_query(f.uwp, &like_to_regex(t)),
        Clause::Pbg(t) => regex_query(f.pbg, &like_to_regex(t)),
        Clause::Zone(t) => regex_query(f.zone, &like_to_regex(t)),
        Clause::Alleg(t) => regex_query(f.alleg, &like_to_regex(t)),
        Clause::Ex(t) => regex_query(f.ex, &like_to_regex(t)),
        Clause::Cx(t) => regex_query(f.cx, &like_to_regex(t)),
        Clause::Ix(n) => term_i64(f.ix, *n as i64),
        // token match: term (as LIKE) must match a whole stellar/remark token —
        // each token is its own term, so a `RegexQuery` over the field suffices.
        Clause::Stellar(t) => regex_query(f.stellar, &like_to_regex(t)),
        Clause::Remark(t) => regex_query(f.remarks, &like_to_regex(t)),
        // sector_name LIKE '%'+term+'%'.
        Clause::InSector(t) => regex_query(f.sector_name, &format!(".*{}.*", like_to_regex(t))),
    }
}

/// A `RegexQuery` over `field` (full-term match); a bad regex → matches nothing.
fn regex_query(field: Field, pattern: &str) -> Box<dyn Query> {
    match RegexQuery::from_pattern(pattern, field) {
        Ok(q) => Box::new(q),
        Err(_) => never_match(),
    }
}

fn term_text(field: Field, text: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(Term::from_field_text(field, text), IndexRecordOption::Basic))
}

fn term_i64(field: Field, v: i64) -> Box<dyn Query> {
    Box::new(TermQuery::new(Term::from_field_i64(field, v), IndexRecordOption::Basic))
}

fn term_u64(field: Field, v: u64) -> Box<dyn Query> {
    Box::new(TermQuery::new(Term::from_field_u64(field, v), IndexRecordOption::Basic))
}

/// A query that matches no documents (empty intersection).
fn never_match() -> Box<dyn Query> {
    Box::new(BooleanQuery::new(Vec::new()))
}
