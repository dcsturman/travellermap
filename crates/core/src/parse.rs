//! Parser for the T5 Second Survey **tab-delimited** sector format (`.tab`).
//!
//! Ported from the reference `TabDelimitedParser`/`ParseWorld`
//! (`server/serialization/SectorParser.cs`). The format is **header-driven**:
//! the first non-comment line names the columns, and each data row is split on
//! tabs and matched to fields *by column name* (with the same alternate names
//! the reference accepts), so column order/presence can vary between files.
//!
//! Lenient by design: structural failure (no header) is an error; a malformed
//! row is skipped and reported in [`ParseOutcome::warnings`] rather than
//! aborting the whole sector — matching the reference, which logs and continues.

use crate::astrometrics::Coord;
use crate::dto::{Border, Route, SectorIndexEntry, SubPath, Subsector, VectorObject, World};
use base64::Engine;
use regex::Regex;
use std::collections::HashMap;

/// Structural parse failure (e.g. the file has no header row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// Result of parsing a `.tab` file: the worlds that parsed, plus any per-row
/// problems that were skipped.
#[derive(Debug, Clone, Default)]
pub struct ParseOutcome {
    pub worlds: Vec<World>,
    pub warnings: Vec<String>,
}

/// Parse a T5 tab-delimited sector file into worlds.
///
/// Skips `#` comment lines and blanks; the first remaining line is the header.
pub fn parse_tab(input: &str) -> Result<ParseOutcome, ParseError> {
    // (1-based line number, content) for every non-comment, non-blank line.
    let mut rows = input
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with('#') && !l.trim().is_empty());

    let (_, header_line) = rows.next().ok_or_else(|| ParseError {
        message: "no header row (file is empty or all comments)".into(),
    })?;
    let header: Vec<&str> = header_line.split('\t').collect();

    let mut out = ParseOutcome::default();
    for (line_no, line) in rows {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != header.len() {
            out.warnings.push(format!(
                "line {line_no}: {} columns, expected {}",
                cols.len(),
                header.len()
            ));
            continue;
        }
        let row: HashMap<&str, &str> = header.iter().copied().zip(cols.iter().copied()).collect();
        let get = |keys: &[&str]| keys.iter().find_map(|k| row.get(k).map(|v| v.to_string()));
        match world_from_row(get) {
            Ok(world) => out.worlds.push(world),
            Err(msg) => out.warnings.push(format!("line {line_no}: {msg}")),
        }
    }
    Ok(out)
}

/// Parse the T5 Second Survey **column-delimited** format (`.txt`/`.sec`):
/// header line, then a `---- ---- …` separator whose dash runs define each
/// column's position, then fixed-width data rows. Ported from the reference
/// `ColumnParser`. Field names match `.tab`, so it reuses `world_from_row`.
pub fn parse_column(input: &str) -> Result<ParseOutcome, ParseError> {
    let mut rows = input
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim_end()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));

    let (_, header) = rows.next().ok_or_else(|| ParseError {
        message: "no header row".into(),
    })?;
    let (_, separator) = rows.next().ok_or_else(|| ParseError {
        message: "no column-separator row".into(),
    })?;
    let header: Vec<char> = header.chars().collect();
    let columns = column_spans(separator, &header);

    let mut out = ParseOutcome::default();
    for (line_no, line) in rows {
        let chars: Vec<char> = line.chars().collect();
        let cell = |start: usize, len: usize| -> String {
            let end = (start + len).min(chars.len());
            if start >= chars.len() {
                String::new()
            } else {
                chars[start..end].iter().collect::<String>().trim().to_owned()
            }
        };
        let row: HashMap<&str, String> = columns
            .iter()
            .map(|(name, start, len)| (name.as_str(), cell(*start, *len)))
            .collect();
        let get = |keys: &[&str]| keys.iter().find_map(|k| row.get(k).cloned());
        match world_from_row(get) {
            Ok(world) => out.worlds.push(world),
            Err(msg) => out.warnings.push(format!("line {line_no}: {msg}")),
        }
    }
    Ok(out)
}

/// Columns `(name, start, len)` from the dash-separator line: each run of `-`
/// is a column at that char position; the name is the header text there.
fn column_spans(separator: &str, header: &[char]) -> Vec<(String, usize, usize)> {
    let sep: Vec<char> = separator.chars().collect();
    let mut cols = Vec::new();
    let mut i = 0;
    while i < sep.len() {
        if sep[i] == '-' {
            let start = i;
            while i < sep.len() && sep[i] == '-' {
                i += 1;
            }
            let len = i - start;
            let end = (start + len).min(header.len());
            let name: String = header
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .collect::<String>()
                .trim()
                .to_owned();
            cols.push((name, start, len));
        } else {
            i += 1;
        }
    }
    cols
}

/// Build a `World` from a field accessor (`get(keys)` → first present value).
/// Shared by the tab and column parsers; field names and alternate names match
/// the reference `ParseWorld`.
/// Parse the **legacy SEC** format (`.sec`) — the old HIWG/early-travellermap
/// layout with a column-ruler comment instead of a parseable header. Ported
/// from the reference `SecParser` (regex-driven): each world line is matched by
/// a single regex (name / hex / UWP / base / remarks / zone / PBG / allegiance /
/// stars). Lines without a UWP (comments, the ruler, the sector name/coords
/// header) are skipped.
pub fn parse_sec(input: &str) -> Result<ParseOutcome, ParseError> {
    use std::sync::OnceLock;
    // Faithful port of the reference `WORLD_REGEX` (IgnorePatternWhitespace
    // expanded into explicit `[ \t]` runs). Greedy name + rigid suffix gives a
    // unique split. `codes` is lazy with a 10-char floor, matching the original.
    static WORLD_RE: OnceLock<Regex> = OnceLock::new();
    static UWP_RE: OnceLock<Regex> = OnceLock::new();
    let world_re = WORLD_RE.get_or_init(|| {
        Regex::new(concat!(
            r"^[ \t]*(?P<name>.*)",
            r"[ \t]*(?P<hex>[0-9]{4})",
            r"[ \t]{1,2}(?P<uwp>[ABCDEX?][0-9A-Z?]{6}-[0-9A-Z?])",
            r"[ \t]{1,2}(?P<base>[A-Zr1-9* -])",
            r"[ \t]{1,2}(?P<codes>.{10,}?)",
            r"(?:[ \t]+(?P<zone>[GARBFU -]))?",
            r"[ \t]{1,2}(?P<pbg>[0-9X?][0-9A-FX?][0-9A-FX?])",
            r"[ \t]{1,2}(?P<allegiance>[A-Za-z0-9][A-Za-z0-9?-]|--)",
            r"[ \t]*(?P<rest>.*?)[ \t]*$",
        ))
        .expect("valid SEC world regex")
    });
    let uwp_re =
        UWP_RE.get_or_init(|| Regex::new(r"[ABCDEX?][0-9A-Z?]{6}-[0-9A-Z?]").expect("valid UWP regex"));

    let mut worlds = Vec::new();
    let mut warnings = Vec::new();
    for (i, line) in input.lines().enumerate() {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with(['#', '$', '@']) {
            continue;
        }
        if !uwp_re.is_match(line) {
            continue; // header / ruler / sector name+coords / non-world line
        }
        let Some(c) = world_re.captures(line) else {
            warnings.push(format!("line {}: SEC parse failed: {line}", i + 1));
            continue;
        };
        let grp = |k| c.name(k).map_or("", |m| m.as_str()).trim();
        let hex = grp("hex").to_string();
        let allegiance = grp("allegiance").to_string();
        // Trailing '.'/'+' fixups, then drop placeholder names.
        let mut name = grp("name").trim_end_matches(['.', '+']).trim().to_string();
        if name == hex || is_placeholder_name(&name) {
            name.clear();
        }
        let base = grp("base");
        let bases = match base.chars().next() {
            Some(c) if c != '-' && c != '*' => decode_legacy_bases(&allegiance, c),
            _ => String::new(),
        };
        let zone = match grp("zone") {
            "-" | "G" | "U" => String::new(), // G/U aren't drawn zones; '-' = none
            z => z.to_string(),
        };
        worlds.push(World {
            hex,
            name,
            uwp: grp("uwp").to_string(),
            bases,
            remarks: grp("codes").to_string(),
            zone,
            pbg: grp("pbg").to_string(),
            allegiance,
            stellar: grp("rest").to_string(),
            importance: None,
            economic: None,
            cultural: None,
            nobility: None,
            worlds: None,
            resource_units: None,
        });
    }
    Ok(ParseOutcome { worlds, warnings })
}

/// SEC placeholder names like `A-1` … `P-99` (reference `PLACEHOLDER_NAME_REGEX`)
/// are blanked.
fn is_placeholder_name(name: &str) -> bool {
    let b = name.as_bytes();
    (3..=4).contains(&b.len())
        && (b'A'..=b'P').contains(&b[0])
        && b[1] == b'-'
        && b[2..].iter().all(u8::is_ascii_digit)
}

/// Translate a legacy single-char base code to modern multi-base letters
/// (reference `SecondSurvey.DecodeLegacyBases` glob table; only `So.`/`Sc.`
/// are allegiance-specific). Unknown codes → empty (no spurious glyph).
fn decode_legacy_bases(allegiance: &str, code: char) -> String {
    let a2 = allegiance.get(..2).unwrap_or(allegiance);
    let s = match (a2, code) {
        ("So", 'F') => "K",
        ("So", 'K') => "KM",
        ("Sc", 'H') => "H",
        (_, '2' | 'A') => "NS",
        (_, 'B') => "NW",
        (_, 'C') => "C",
        (_, 'D' | 'Y') => "D",
        (_, 'E') => "E",
        (_, 'F') => "KM",
        (_, 'G' | 'J' | 'K' | 'L' | 'P') => "K",
        (_, 'H') => "CK",
        (_, 'M' | 'Q') => "M",
        (_, 'N') => "N",
        (_, 'O') => "O",
        (_, 'R') => "R",
        (_, 'S') => "S",
        (_, 'T') => "T",
        (_, 'U') => "RT",
        (_, 'V') => "V",
        (_, 'W' | 'X') => "W",
        (_, 'Z') => "KM",
        (_, 'I') => "I",
        _ => "",
    };
    s.to_string()
}

fn world_from_row(get: impl Fn(&[&str]) -> Option<String>) -> Result<World, String> {
    // Hex + UWP identify a world; without them the row isn't one.
    let hex = get(&["Hex"]).unwrap_or_default();
    let uwp = get(&["UWP"]).unwrap_or_default();
    if hex.is_empty() || uwp.is_empty() {
        return Err("missing Hex or UWP".into());
    }
    let dash = |s: String| if s == "-" { String::new() } else { s };
    let nonempty = |o: Option<String>| o.filter(|s| !s.is_empty());

    Ok(World {
        hex,
        name: get(&["Name"]).unwrap_or_default(),
        uwp,
        bases: dash(get(&["B", "Bases"]).unwrap_or_default()),
        remarks: dash(get(&["Remarks", "Trade Codes", "Comments"]).unwrap_or_default()),
        zone: dash(get(&["Z", "Zone"]).unwrap_or_default()),
        pbg: get(&["PBG"]).unwrap_or_default(),
        allegiance: get(&["A", "Al", "Allegiance"]).unwrap_or_default(),
        stellar: get(&["Stellar", "Stars", "Stellar Data"]).unwrap_or_default(),
        importance: nonempty(get(&["{Ix}", "{ Ix }", "Ix"])),
        economic: nonempty(get(&["(Ex)", "( Ex )", "Ex"])),
        cultural: nonempty(get(&["[Cx]", "[ Cx ]", "Cx"])),
        nobility: nonempty(get(&["N", "Nobility"]).map(dash)),
        worlds: get(&["W", "Worlds"]).and_then(|v| v.trim().parse().ok()),
        resource_units: get(&["RU"]).and_then(|v| v.replace(',', "").trim().parse().ok()),
    })
}

/// Parse the head of a sector metadata `.xml` (`<Sector>` root) into an index
/// entry — name, abbreviation, grid `(X, Y)`. Ignores the rest (borders,
/// routes, etc., parsed in a later phase).
///
/// Returns `Err` for files whose root isn't a `<Sector>` with `X`/`Y`/`Name`
/// (e.g. the milieu-level region-name list), which lets the backend skip
/// non-sector XML while scanning a milieu directory.
pub fn sector_index_entry(xml: &str) -> Result<SectorIndexEntry, ParseError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| ParseError {
        message: e.to_string(),
    })?;
    let root = doc.root_element();
    if !root.has_tag_name("Sector") {
        return Err(ParseError {
            message: format!("root is <{}>, not <Sector>", root.tag_name().name()),
        });
    }

    let child_int = |tag: &str| {
        root.children()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<i32>().ok())
    };
    let (x, y) = match (child_int("X"), child_int("Y")) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            return Err(ParseError {
                message: "missing <X>/<Y>".into(),
            })
        }
    };
    // First <Name> child (the canonical name; localized `<Name Lang=...>` follow).
    let name = root
        .children()
        .find(|n| n.has_tag_name("Name"))
        .and_then(|n| n.text())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ParseError {
            message: "missing <Name>".into(),
        })?;

    Ok(SectorIndexEntry {
        name,
        abbreviation: root.attribute("Abbreviation").map(str::to_owned),
        location: Coord::new(x, y),
        data_file: None,
        data_format: None,
        metadata_file: None,
    })
}

/// Parse a milieu **region list** (e.g. `M1105.xml`) into index entries —
/// `<Sector>` elements with `<X>/<Y>/<Name>` and a `<DataFile Type="…">file</>`.
/// This is the authoritative source: it covers sectors whose per-sector `.xml`
/// omits coords, and gives the exact data file + format to load. Entries
/// without a `DataFile` (named regions with no data) are skipped.
pub fn parse_milieu_index(xml: &str) -> Vec<SectorIndexEntry> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("Sector"))
        .filter_map(|sector| {
            let child_int = |tag: &str| {
                sector
                    .children()
                    .find(|n| n.has_tag_name(tag))
                    .and_then(|n| n.text())
                    .and_then(|t| t.trim().parse::<i32>().ok())
            };
            let (x, y) = (child_int("X")?, child_int("Y")?);
            let name = sector
                .children()
                .find(|n| n.has_tag_name("Name"))
                .and_then(|n| n.text())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())?;
            let data = sector.children().find(|n| n.has_tag_name("DataFile"))?;
            let data_file = data.text().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())?;
            let metadata_file = sector
                .children()
                .find(|n| n.has_tag_name("MetadataFile"))
                .and_then(|n| n.text())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            Some(SectorIndexEntry {
                name,
                abbreviation: sector.attribute("Abbreviation").map(str::to_owned),
                location: Coord::new(x, y),
                data_format: data.attribute("Type").map(str::to_owned),
                data_file: Some(data_file),
                metadata_file,
            })
        })
        .collect()
}

/// Named subsectors from a sector metadata `.xml` (`<Subsector Index="A">Name</Subsector>`).
/// Entries with an empty name are skipped. Returns empty if there's no
/// `<Subsectors>` block.
pub fn sector_subsectors(xml: &str) -> Vec<Subsector> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("Subsector"))
        .filter_map(|n| {
            let index = n.attribute("Index")?.trim().to_owned();
            let name = n.text().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())?;
            Some(Subsector { index, name })
        })
        .collect()
}

/// A capital/homeworld marker from `res/labels/Worlds.xml`, *before* its sector
/// name is resolved to coordinates (the backend does that — it holds the sector
/// map; this crate stays I/O-free). `bias_*` default to +1 per the reference
/// `WorldObject` (LabelBiasX/Y ∈ {−1, 0, +1}).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldLabelDef {
    pub name: String,
    pub sector: String,
    pub hex: String,
    pub bias_x: i8,
    pub bias_y: i8,
}

/// Parse `res/labels/Worlds.xml` into capital/homeworld marker definitions
/// (`<World><Name/><Location Sector= Hex=/><LabelBiasX/><LabelBiasY/></World>`).
pub fn parse_world_labels(xml: &str) -> Vec<WorldLabelDef> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("World"))
        .filter_map(|n| {
            let child_text = |tag| {
                n.children()
                    .find(|c| c.has_tag_name(tag))
                    .and_then(|c| c.text())
                    .map(str::trim)
            };
            let name = child_text("Name").filter(|s| !s.is_empty())?.to_owned();
            let loc = n.children().find(|c| c.has_tag_name("Location"))?;
            let bias = |tag| child_text(tag).and_then(|s| s.parse::<i8>().ok());
            Some(WorldLabelDef {
                name,
                sector: loc.attribute("Sector")?.trim().to_owned(),
                hex: loc.attribute("Hex")?.trim().to_owned(),
                bias_x: bias("LabelBiasX").unwrap_or(1),
                bias_y: bias("LabelBiasY").unwrap_or(1),
            })
        })
        .collect()
}

/// The root `<Sector>` element's `Tags` attribute — space-separated review tags
/// (`Official Preserve InReview Unreviewed Apocryphal`). Returns it trimmed, or
/// an empty string if the root isn't `<Sector>`, the attribute is absent, or the
/// XML doesn't parse.
pub fn sector_tags(xml: &str) -> String {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return String::new();
    };
    doc.root_element()
        .attribute("Tags")
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// The `<Credits>` child of the root `<Sector>` element, as trimmed text
/// (roxmltree decodes XML entities, so `&lt;b&gt;` arrives as `<b>`). Returns
/// `None` if absent, empty, or the XML doesn't parse.
pub fn sector_credits(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let credits = doc.root_element().children().find(|n| n.has_tag_name("Credits"))?;
    // Concatenate all descendant text so multi-fragment content isn't truncated.
    let text: String = credits.descendants().filter_map(|n| n.text()).collect();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Per-sector allegiance borders from a sector metadata `.xml` (`<Border
/// Allegiance="…">hex hex …</Border>`).
pub fn sector_borders(xml: &str) -> Vec<Border> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("Border"))
        .filter_map(|n| {
            let allegiance = n.attribute("Allegiance")?.trim().to_owned();
            let hexes: Vec<String> = n
                .text()
                .unwrap_or("")
                .split_whitespace()
                .filter(|t| t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()))
                .map(str::to_owned)
                .collect();
            if hexes.is_empty() {
                return None;
            }
            Some(Border {
                allegiance,
                hexes,
                region: Vec::new(),
                color: n.attribute("Color").map(str::to_owned),
                label: n.attribute("Label").map(str::to_owned),
                label_position: n.attribute("LabelPosition").map(str::to_owned),
            })
        })
        .collect()
}

/// Per-sector border colors from the embedded `<Stylesheet>` (CSS-like:
/// `border.SwCf { color: blue; }`). Returns allegiance → color, which overrides
/// the global `otu.css` table but is overridden by a border's explicit `Color`.
pub fn sector_border_styles(xml: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return map;
    };
    let Some(text) = doc
        .descendants()
        .find(|n| n.has_tag_name("Stylesheet"))
        .and_then(|n| n.text())
    else {
        return map;
    };
    for rule in text.split('}') {
        let Some((selectors, body)) = rule.split_once('{') else {
            continue;
        };
        let color = body.split(';').find_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            k.trim().eq_ignore_ascii_case("color").then(|| v.trim().to_owned())
        });
        let Some(color) = color else { continue };
        for sel in selectors.split(',') {
            if let Some(alleg) = sel.trim().strip_prefix("border.") {
                map.insert(alleg.trim().to_owned(), color.clone());
            }
        }
    }
    map
}

/// Even/odd column stagger (even columns shifted +0.5 row), matching the
/// client's `hex_parsec`.
fn stagger(wc: i32) -> f64 {
    if wc.rem_euclid(2) == 0 {
        0.5
    } else {
        0.0
    }
}

/// Even-odd point-in-polygon test.
fn point_in_poly(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// All **absolute** world hexes inside a border: its boundary hexes plus the
/// interior (hexes whose center falls within the boundary polygon). `(sx, sy)`
/// is the sector's grid position. Used to render exact hex-edge borders.
pub fn border_region(hexes: &[String], sx: i32, sy: i32) -> Vec<(i32, i32)> {
    const W: i32 = 32;
    const H: i32 = 40;
    // The polygon uses ALL boundary points (including off-sector marker hexes
    // like col 0/33 or row 0/41 that signal the border continues across a
    // seam), but the filled region is clipped to this sector's hexes so it
    // never spills into a neighbor (which would double-fill at the boundary).
    let mut poly = Vec::new();
    let mut bset = std::collections::HashSet::new();
    for hx in hexes {
        if let Some((col, row)) = crate::astrometrics::parse_hex(hx) {
            let (wc, wr) = (sx * W + col, sy * H + row);
            poly.push((wc as f64, wr as f64 + stagger(wc)));
            if (1..=W).contains(&col) && (1..=H).contains(&row) {
                bset.insert((wc, wr));
            }
        }
    }
    if poly.len() < 3 {
        return bset.into_iter().collect();
    }
    let mut region = Vec::new();
    for wc in (sx * W + 1)..=(sx * W + W) {
        for wr in (sy * H + 1)..=(sy * H + H) {
            if bset.contains(&(wc, wr)) || point_in_poly((wc as f64, wr as f64 + stagger(wc)), &poly) {
                region.push((wc, wr));
            }
        }
    }
    region
}

/// Extract one sector's inline metadata block (`<Sector>…</Sector>`) from a
/// milieu region list, by sector name. Many sectors keep their borders/routes/
/// subsectors inline in the region list (`{milieu}.xml`) in addition to — or
/// instead of — a per-sector `<name>.xml` (the Aslan Hierate interior has only
/// the inline form). The returned substring is a valid single-root XML document
/// that `sector_borders`/`sector_routes`/`sector_subsectors`/`sector_border_styles`
/// consume directly. Matches against any `<Name>` child (sectors can carry
/// several localized names).
pub fn milieu_sector_block(region_xml: &str, name: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(region_xml).ok()?;
    let node = doc.descendants().filter(|n| n.has_tag_name("Sector")).find(|s| {
        s.children()
            .filter(|n| n.has_tag_name("Name"))
            .any(|n| n.text().map(|t| t.trim() == name).unwrap_or(false))
    })?;
    Some(region_xml[node.range()].to_owned())
}

/// Per-sector routes from a sector metadata `.xml` (`<Route Start="…" End="…"
/// [StartOffsetX/Y EndOffsetX/Y Allegiance]/>`).
pub fn sector_routes(xml: &str) -> Vec<Route> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let off = |n: &roxmltree::Node, a: &str| n.attribute(a).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    doc.descendants()
        .filter(|n| n.has_tag_name("Route"))
        .filter_map(|n| {
            Some(Route {
                start: n.attribute("Start")?.trim().to_owned(),
                end: n.attribute("End")?.trim().to_owned(),
                start_offset: (off(&n, "StartOffsetX"), off(&n, "StartOffsetY")),
                end_offset: (off(&n, "EndOffsetX"), off(&n, "EndOffsetY")),
                allegiance: n.attribute("Allegiance").map(str::to_owned),
            })
        })
        .collect()
}

/// Parse a `res/Vectors/*.xml` macro overlay (`<VectorObject>` root) into a
/// [`VectorObject`]. `OriginX/Y` default to 0 and `ScaleX/Y` to 1 if absent
/// (some files, e.g. routes, omit the origin).
pub fn parse_vector_object(xml: &str) -> Result<VectorObject, ParseError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| ParseError {
        message: e.to_string(),
    })?;
    let root = doc.root_element();

    let child_f32 = |tag: &str, default: f32| {
        root.children()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<f32>().ok())
            .unwrap_or(default)
    };
    let child_text = |tag: &str| {
        root.children()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(|s| s.trim().to_owned())
    };

    let mut points = Vec::new();
    if let Some(pdp) = root.children().find(|n| n.has_tag_name("PathDataPoints")) {
        for pt in pdp.children().filter(|n| n.has_tag_name("PointF")) {
            let coord = |tag: &str| {
                pt.children()
                    .find(|n| n.has_tag_name(tag))
                    .and_then(|n| n.text())
                    .and_then(|t| t.trim().parse::<f32>().ok())
            };
            if let (Some(x), Some(y)) = (coord("X"), coord("Y")) {
                points.push((x, y));
            }
        }
    }

    // GDI+ point-type bytes (base64), one per point: low bits give the type
    // (0 = Start = begin a new sub-path), high bit 0x80 = CloseSubpath.
    let types = child_text("PathDataTypes")
        .map(|s| {
            let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(cleaned)
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let origin = (child_f32("OriginX", 0.0), child_f32("OriginY", 0.0));
    let scale = (child_f32("ScaleX", 1.0), child_f32("ScaleY", 1.0));
    let name_off = (child_f32("NameX", 0.0), child_f32("NameY", 0.0));

    // Label anchor from <Bounds> (transformed like the reference NamePosition).
    let label = root
        .children()
        .find(|n| n.has_tag_name("Bounds"))
        .and_then(|b| {
            let f = |tag: &str| {
                b.children()
                    .find(|n| n.has_tag_name(tag))
                    .and_then(|n| n.text())
                    .and_then(|t| t.trim().parse::<f32>().ok())
            };
            Some((f("X")?, f("Y")?, f("Width")?, f("Height")?))
        })
        .map(|(bx, by, bw, bh)| name_anchor(bx, by, bw, bh, origin, scale, name_off));

    Ok(VectorObject {
        name: child_text("Name").unwrap_or_default(),
        map_options: child_text("MapOptions"),
        origin,
        scale,
        paths: split_subpaths(points, &types),
        label,
    })
}

/// Label position in world space from raw bounds, mirroring the reference
/// `VectorObject.NamePosition`: center of the transformed bounds, shifted by
/// `NameX/NameY` as a fraction of the (raw) bounds size.
fn name_anchor(
    bx: f32,
    by: f32,
    bw: f32,
    bh: f32,
    origin: (f32, f32),
    scale: (f32, f32),
    name_off: (f32, f32),
) -> (f32, f32) {
    // Transformed bounds, normalized for negative scale.
    let (mut tx, mut tw) = ((bx - origin.0) * scale.0, bw * scale.0);
    let (mut ty, mut th) = ((by - origin.1) * scale.1, bh * scale.1);
    if tw < 0.0 {
        tx += tw;
        tw = -tw;
    }
    if th < 0.0 {
        ty += th;
        th = -th;
    }
    let cx = tx + tw / 2.0 + if bw != 0.0 { tw * (name_off.0 / bw) } else { 0.0 };
    let cy = ty + th / 2.0 + if bh != 0.0 { th * (name_off.1 / bh) } else { 0.0 };
    (cx, cy)
}

/// Split a flat point list into sub-paths using GDI+ point-type bytes. A byte
/// whose low 3 bits are 0 (`Start`) begins a new sub-path; the `0x80`
/// (`CloseSubpath`) flag closes the one it terminates. With no/short type data,
/// the whole list is one open sub-path.
fn split_subpaths(points: Vec<(f32, f32)>, types: &[u8]) -> Vec<SubPath> {
    let mut paths = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut closed = false;
    for (i, p) in points.into_iter().enumerate() {
        let t = types.get(i).copied().unwrap_or(1); // default Line
        if (t & 0x07) == 0 && !cur.is_empty() {
            paths.push(SubPath {
                points: std::mem::take(&mut cur),
                closed,
            });
            closed = false;
        }
        cur.push(p);
        if t & 0x80 != 0 {
            closed = true;
        }
    }
    if !cur.is_empty() {
        paths.push(SubPath { points: cur, closed });
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    // A faithful slice of the M1105 Spinward Marches `.tab` (header + rows),
    // including the leading comment lines the real files carry.
    const SAMPLE: &str = "\
# Generated file - DO NOT MODIFY
# Update source files in res/t5ss/data instead

Sector\tSS\tHex\tName\tUWP\tBases\tRemarks\tZone\tPBG\tAllegiance\tStars\t{Ix}\t(Ex)\t[Cx]\tNobility\tW\tRU
Spin\tA\t0101\tZeycude\tC430698-9\t\tDe Na Ni Po\t\t613\tZhIN\tK9 V\t{ -1 }\t(C53-1)\t[6559]\t\t8\t-180
Spin\tA\t1910\tRegina\tA788899-C\tNS\tRi Pa Ph An Cp\t\t703\tImDd\tG2 V\t{ 4 }\t(D7E+5)\t[9C6D]\tBcCeF\t9\t4628
Spin\tA\t0102\tReno\tT BADROW MISSING COLS";

    #[test]
    fn parses_header_and_rows() {
        let out = parse_tab(SAMPLE).expect("has header");
        assert_eq!(out.worlds.len(), 2);

        let regina = out.worlds.iter().find(|w| w.name == "Regina").unwrap();
        assert_eq!(regina.hex, "1910");
        assert_eq!(regina.uwp, "A788899-C");
        assert_eq!(regina.bases, "NS");
        assert_eq!(regina.allegiance, "ImDd");
        assert_eq!(regina.pbg, "703");
        assert_eq!(regina.stellar, "G2 V");
        assert_eq!(regina.importance.as_deref(), Some("{ 4 }"));
        assert_eq!(regina.cultural.as_deref(), Some("[9C6D]"));
        assert_eq!(regina.nobility.as_deref(), Some("BcCeF"));
        assert_eq!(regina.worlds, Some(9));
        assert_eq!(regina.resource_units, Some(4628));
        assert_eq!(regina.codes().collect::<Vec<_>>(), ["Ri", "Pa", "Ph", "An", "Cp"]);
    }

    #[test]
    fn empty_fields_stay_empty_and_optional_absent() {
        let out = parse_tab(SAMPLE).unwrap();
        let zeycude = out.worlds.iter().find(|w| w.name == "Zeycude").unwrap();
        assert_eq!(zeycude.bases, "");
        assert_eq!(zeycude.zone, "");
        assert_eq!(zeycude.nobility, None);
    }

    #[test]
    fn malformed_row_is_skipped_with_warning() {
        let out = parse_tab(SAMPLE).unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("columns"));
    }

    #[test]
    fn no_header_is_an_error() {
        assert!(parse_tab("# only comments\n\n").is_err());
    }

    #[test]
    fn parses_column_format() {
        // Fixed-width T5 Second Survey (the Mikhail.txt shape): header, a
        // dash-separator defining columns, then space-aligned rows.
        let input = "\
Hex  Name                 UWP       Remarks       {Ix}   PBG W  A
---- -------------------- --------- ------------- ------ --- -- ----
0102 Miuir                B130A88-E De Hi Na Po   {  3 } 900  5 NaXX
0104                      B638416-A Ni            {  1 } 222 11 NaXX";
        let out = parse_column(input).expect("valid");
        assert_eq!(out.worlds.len(), 2);
        let m = &out.worlds[0];
        assert_eq!(m.name, "Miuir");
        assert_eq!(m.hex, "0102");
        assert_eq!(m.uwp, "B130A88-E");
        assert_eq!(m.remarks, "De Hi Na Po");
        assert_eq!(m.pbg, "900");
        assert_eq!(m.allegiance, "NaXX");
        assert_eq!(m.importance.as_deref(), Some("{  3 }"));
        assert_eq!(out.worlds[1].name, ""); // unnamed world
        assert_eq!(out.worlds[1].hex, "0104");
    }

    #[test]
    fn parses_sector_metadata_head() {
        let xml = r#"<?xml version="1.0"?>
            <Sector Abbreviation="Spin" Tags="Official">
              <Y>-1</Y>
              <X>-4</X>
              <Name>Spinward Marches</Name>
              <Name Lang="zh">Tloql</Name>
            </Sector>"#;
        let e = sector_index_entry(xml).unwrap();
        assert_eq!(e.name, "Spinward Marches");
        assert_eq!(e.abbreviation.as_deref(), Some("Spin"));
        assert_eq!(e.location, Coord::new(-4, -1));
    }

    #[test]
    fn non_sector_xml_is_rejected() {
        assert!(sector_index_entry("<Milieu><Sector><X>0</X></Sector></Milieu>").is_err());
    }

    #[test]
    fn reads_sector_tags_attribute() {
        let xml = r#"<Sector Abbreviation="Spin" Tags="Official">
              <X>-4</X><Y>-1</Y><Name>Spinward Marches</Name>
            </Sector>"#;
        assert_eq!(sector_tags(xml), "Official");
        // Absent → empty.
        assert_eq!(sector_tags(r#"<Sector><Name>X</Name></Sector>"#), "");
    }

    #[test]
    fn reads_credits_decoded_text() {
        // Mirrors res/Sectors/M1248/1248_Akti.xml: entity-encoded HTML inside
        // <Credits>; roxmltree decodes the entities.
        let xml = r#"<Sector>
              <Name>Aktifao</Name>
              <Credits>
                &lt;b&gt;Aktifao&lt;/b&gt; sector borders designed by Shane MacLean.
              </Credits>
            </Sector>"#;
        let c = sector_credits(xml).expect("has credits");
        assert!(c.starts_with("<b>Aktifao</b>"), "decoded + trimmed: {c:?}");
        // Absent → None.
        assert_eq!(sector_credits(r#"<Sector><Name>X</Name></Sector>"#), None);
    }

    #[test]
    fn parses_vector_object() {
        // Shape of res/Vectors/CoreRoute.xml (route: no origin, negative ScaleY).
        let xml = r#"<?xml version="1.0"?>
            <VectorObject>
              <Name>Zhodani Core Route</Name>
              <MapOptions>BordersMinor NamesMinor</MapOptions>
              <ScaleX>1.152</ScaleX>
              <ScaleY>-1</ScaleY>
              <PathDataPoints>
                <PointF><X>-172</X><Y>360</Y></PointF>
                <PointF><X>-170</X><Y>7000</Y></PointF>
              </PathDataPoints>
            </VectorObject>"#;
        let v = parse_vector_object(xml).unwrap();
        assert_eq!(v.name, "Zhodani Core Route");
        assert_eq!(v.origin, (0.0, 0.0)); // absent → default
        assert_eq!(v.scale, (1.152, -1.0));
        // No PathDataTypes → one open sub-path with all points.
        assert_eq!(v.paths.len(), 1);
        assert!(!v.paths[0].closed);
        assert_eq!(v.paths[0].points, vec![(-172.0, 360.0), (-170.0, 7000.0)]);
    }

    #[test]
    fn splits_disjoint_subpaths_by_type_bytes() {
        // Two separate closed triangles. Type bytes: 0x00 Start, 0x01 Line,
        // 0x81 Line+Close — repeated. Base64 of [0,1,0x81, 0,1,0x81]:
        let types_b64 = base64::engine::general_purpose::STANDARD.encode([0u8, 1, 0x81, 0, 1, 0x81]);
        let xml = format!(
            r#"<VectorObject><Name>Two Regions</Name><ScaleX>1</ScaleX><ScaleY>1</ScaleY>
                <PathDataPoints>
                  <PointF><X>0</X><Y>0</Y></PointF>
                  <PointF><X>1</X><Y>0</Y></PointF>
                  <PointF><X>0</X><Y>1</Y></PointF>
                  <PointF><X>9</X><Y>9</Y></PointF>
                  <PointF><X>9</X><Y>8</Y></PointF>
                  <PointF><X>8</X><Y>9</Y></PointF>
                </PathDataPoints>
                <PathDataTypes>{types_b64}</PathDataTypes></VectorObject>"#
        );
        let v = parse_vector_object(&xml).unwrap();
        assert_eq!(v.paths.len(), 2, "two disjoint regions, not one");
        assert!(v.paths.iter().all(|p| p.closed && p.points.len() == 3));
    }
}
