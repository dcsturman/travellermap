//! World-table serializers for the `/api/sec` endpoint — ports of the reference
//! `server/serialization/SectorWriter.cs` (`TabDelimitedSerializer`,
//! `SecondSurveySerializer`) and the `ColumnSerializer` they rely on
//! (`ColumnUtils.cs`).
//!
//! These emit the **world rows** (and, for SecondSurvey, the aligned column
//! header). The leading `# …` metadata comment block that SEC/SecondSurvey carry
//! lives in the backend handler (it needs sector metadata, not just worlds). All
//! line endings are CRLF, matching the reference (`StreamWriter.WriteLine` on the
//! Windows host) so output is byte-identical.
//!
//! The legacy fixed-column SEC format (`SecSerializer`) is intentionally not
//! ported yet — it needs the T5→legacy allegiance/base transforms.

use crate::astrometrics::{parse_hex, subsector_hex, subsector_index, subsector_letter};
use crate::dto::World;

/// The reference enumerates a `WorldCollection` column-major (x outer, y inner)
/// and then stable-`OrderBy`s by subsector, so the serialized order is
/// `(subsector_index, col, row)`. Sorting by this key reproduces it regardless
/// of the parsed file's row order.
fn order_key(w: &World) -> (usize, i32, i32) {
    let (col, row) = parse_hex(&w.hex).unwrap_or((0, 0));
    (subsector_index(&w.hex), col, row)
}

/// Reference uses `Environment.NewLine` (CRLF) on its Windows host.
const NL: &str = "\r\n";

/// Knobs shared by the writers (subset of the reference `SectorSerializeOptions`
/// that affects the world table; metadata/header-comment are handled upstream).
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteOptions {
    /// Emit hexes relative to their subsector (`sscoords=1`) instead of sector hexes.
    pub sscoords: bool,
    /// Emit the column header (+ separator row for SecondSurvey).
    pub include_header: bool,
}

fn hex_of(world: &World, opts: &WriteOptions) -> String {
    if opts.sscoords {
        subsector_hex(&world.hex)
    } else {
        world.hex.clone()
    }
}

fn dash_if_empty(s: &str) -> String {
    if s.trim().is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

fn w_field(world: &World) -> String {
    match world.worlds {
        Some(n) if n > 0 => n.to_string(),
        _ => String::new(),
    }
}

// --- TabDelimited --------------------------------------------------------

/// Tab-separated sector data (`TabDelimitedSerializer`). Columns:
/// `Sector SS Hex Name UWP Bases Remarks Zone PBG Allegiance Stars {Ix} (Ex) [Cx] Nobility W RU`.
/// Worlds are ordered by subsector index (stable), matching the reference.
pub fn write_tab(worlds: &[World], sector_abbreviation: &str, opts: &WriteOptions) -> String {
    let mut out = String::new();
    if opts.include_header {
        out.push_str(
            "Sector\tSS\tHex\tName\tUWP\tBases\tRemarks\tZone\tPBG\tAllegiance\tStars\t{Ix}\t(Ex)\t[Cx]\tNobility\tW\tRU",
        );
        out.push_str(NL);
    }

    let mut ordered: Vec<&World> = worlds.iter().collect();
    ordered.sort_by_key(|w| order_key(w));

    for w in ordered {
        let row = [
            sector_abbreviation.to_string(),
            subsector_letter(&w.hex).to_string(),
            hex_of(w, opts),
            w.name.clone(),
            w.uwp.clone(),
            w.bases.clone(),
            w.remarks.clone(),
            w.zone.clone(),
            w.pbg.clone(),
            w.allegiance.clone(),
            w.stellar.clone(),
            w.importance.clone().unwrap_or_default(),
            w.economic.clone().unwrap_or_default(),
            w.cultural.clone().unwrap_or_default(),
            w.nobility.clone().unwrap_or_default(),
            w_field(w),
            w.resource_units.map(|r| r.to_string()).unwrap_or_default(),
        ];
        out.push_str(&row.join("\t"));
        out.push_str(NL);
    }
    out
}

// --- SecondSurvey (aligned columns) --------------------------------------

/// T5 Second Survey columnar text (`SecondSurveySerializer`). Worlds ordered by
/// subsector letter (stable). Column widths auto-size to content with `Name`/
/// `Remarks` floored at 20.
pub fn write_second_survey(worlds: &[World], opts: &WriteOptions) -> String {
    let mut table = ColumnSerializer::new(&[
        "Hex", "Name", "UWP", "Remarks", "{Ix}", "(Ex)", "[Cx]", "N", "B", "Z", "PBG", "W", "A",
        "Stellar",
    ]);
    table.set_minimum_width("Name", 20);
    table.set_minimum_width("Remarks", 20);

    let mut ordered: Vec<&World> = worlds.iter().collect();
    ordered.sort_by_key(|w| order_key(w));

    for w in ordered {
        table.add_row(vec![
            hex_of(w, opts),
            w.name.clone(),
            w.uwp.clone(),
            w.remarks.clone(),
            w.importance.clone().unwrap_or_default(),
            w.economic.clone().unwrap_or_default(),
            w.cultural.clone().unwrap_or_default(),
            dash_if_empty(w.nobility.as_deref().unwrap_or("")),
            dash_if_empty(&w.bases),
            dash_if_empty(&w.zone),
            w.pbg.clone(),
            w_field(w),
            w.allegiance.clone(),
            w.stellar.clone(),
        ]);
    }
    table.serialize(opts.include_header)
}

/// Port of the reference `ColumnSerializer`: fixed-width, space-delimited columns
/// with a dashed separator row under the header. Cells are trimmed; each column
/// is padded (trailing spaces) to its widest cell, floored at any set minimum —
/// including the final column, so rows carry trailing padding exactly as the
/// reference emits.
struct ColumnSerializer {
    rows: Vec<Vec<String>>,
    minimums: Vec<usize>,
    names: Vec<String>,
}

impl ColumnSerializer {
    fn new(header: &[&str]) -> Self {
        let names: Vec<String> = header.iter().map(|h| h.trim().to_string()).collect();
        let minimums = vec![0; names.len()];
        ColumnSerializer {
            rows: vec![names.clone()],
            minimums,
            names,
        }
    }

    fn set_minimum_width(&mut self, col: &str, width: usize) {
        if let Some(i) = self.names.iter().position(|n| n == col) {
            self.minimums[i] = width;
        }
    }

    fn add_row(&mut self, data: Vec<String>) {
        assert_eq!(data.len(), self.names.len(), "differing column counts");
        self.rows.push(data.into_iter().map(|c| c.trim().to_string()).collect());
    }

    fn widths(&self) -> Vec<usize> {
        let mut widths = vec![0usize; self.names.len()];
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.len());
            }
        }
        for (i, m) in self.minimums.iter().enumerate() {
            widths[i] = widths[i].max(*m);
        }
        widths
    }

    fn serialize(&self, include_header: bool) -> String {
        let widths = self.widths();
        let mut out = String::new();
        let render = |out: &mut String, row: &[String]| {
            for (i, cell) in row.iter().enumerate() {
                if i != 0 {
                    out.push(' ');
                }
                out.push_str(cell);
                out.push_str(&" ".repeat(widths[i] - cell.len()));
            }
            out.push_str(NL);
        };

        let start = if include_header { 0 } else { 1 };
        for (idx, row) in self.rows.iter().enumerate().skip(start) {
            render(&mut out, row);
            if idx == 0 {
                // Dashed separator row directly under the header.
                for (i, w) in widths.iter().enumerate() {
                    if i != 0 {
                        out.push(' ');
                    }
                    out.push_str(&"-".repeat(*w));
                }
                out.push_str(NL);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(hex: &str, name: &str) -> World {
        World {
            hex: hex.to_string(),
            name: name.to_string(),
            uwp: "X000000-0".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn tab_is_crlf_with_header_and_sector_ss_columns() {
        let worlds = [world("0101", "A"), world("0908", "B")];
        let out = write_tab(&worlds, "Spin", &WriteOptions { include_header: true, sscoords: false });
        let lines: Vec<&str> = out.split("\r\n").collect();
        assert!(lines[0].starts_with("Sector\tSS\tHex\t"));
        // 0101 → subsector A, 0908 → subsector B; both tagged with the abbrev.
        assert!(lines[1].starts_with("Spin\tA\t0101\tA\t"), "{:?}", lines[1]);
        assert!(lines[2].starts_with("Spin\tB\t0908\tB\t"), "{:?}", lines[2]);
    }

    #[test]
    fn worlds_sort_by_subsector_then_column_major() {
        // Out-of-order input: same subsector A, plus one in B. Expect A worlds in
        // (col,row) order, then B.
        let worlds = [
            world("0203", "second-col"),
            world("0102", "first-col-row2"),
            world("0101", "first-col-row1"),
            world("0901", "subsector-B"),
        ];
        let out = write_tab(&worlds, "X", &WriteOptions { include_header: false, sscoords: false });
        let names: Vec<&str> = out.lines().map(|l| l.split('\t').nth(3).unwrap()).collect();
        assert_eq!(names, ["first-col-row1", "first-col-row2", "second-col", "subsector-B"]);
    }

    #[test]
    fn second_survey_pads_columns_and_floors_name_width() {
        let opts = WriteOptions { include_header: true, ..Default::default() };
        let out = write_second_survey(&[world("0101", "Reno")], &opts);
        let mut lines = out.lines();
        assert!(lines.next().unwrap().starts_with("Hex  Name"));
        // The dashed separator's Name column is floored at 20 dashes even though
        // the only name ("Reno") is 4 chars.
        let sep_fields: Vec<&str> = lines.next().unwrap().split(' ').collect();
        assert_eq!(sep_fields[1], "-".repeat(20));
    }
}
