//! Backend jump-route support: the per-milieu world index + start/end
//! resolution, bridging the pure `tmap_core::route` A* to `res/` data.
//!
//! The reference `RouteHandler` resolves "Sector hhhh" endpoints against the
//! in-memory sector map and runs `PathFinder` over worlds reachable via a
//! `HexSelector`. Here we load every world of a milieu once into a flat,
//! coordinate-keyed index (small enough to keep in RAM — see CLAUDE.md) and run
//! the core A* over it.

use std::collections::HashMap;
use std::path::Path as FsPath;

use tmap_core::astrometrics::{parse_hex, subsector_letter, Coord};
use tmap_core::dto::{RouteResult, RouteWaypoint, Universe};
use tmap_core::parse::sector_subsectors;
use tmap_core::route::{find_route, path_parsecs, RouteOptions, RouteWorld};

use crate::resolve_and_parse_worlds;

/// One world in the route index — the core node plus the metadata needed to
/// build a [`RouteWaypoint`] for the response.
struct IndexedWorld {
    node: RouteWorld,
    name: String,
    hex: String,
    sector: String,
    subsector: String, // subsector display name for the world's hex (may be empty)
    uwp: String,
    pbg: String,
    zone: String,
    allegiance: String, // full display name, resolved from the code
}

/// Every world of a milieu, ready for jump-route finding. `nodes` mirrors
/// `worlds` one-to-one (the slice the core A* operates on).
pub struct WorldIndex {
    worlds: Vec<IndexedWorld>,
    nodes: Vec<RouteWorld>,
}

impl WorldIndex {
    /// Run the core A* and assemble the [`RouteResult`].
    pub fn find_route(
        &self,
        start: usize,
        end: usize,
        jump: i32,
        opts: RouteOptions,
    ) -> Option<RouteResult> {
        let path = find_route(&self.nodes, start, end, jump, opts)?;
        let parsecs = path_parsecs(&self.nodes, &path);
        let waypoints = path
            .iter()
            .map(|&i| {
                let w = &self.worlds[i];
                RouteWaypoint {
                    name: w.name.clone(),
                    hex: w.hex.clone(),
                    coord: w.node.coord,
                    sector: w.sector.clone(),
                    subsector: w.subsector.clone(),
                    uwp: w.uwp.clone(),
                    pbg: w.pbg.clone(),
                    zone: w.zone.clone(),
                    allegiance: w.allegiance.clone(),
                }
            })
            .collect::<Vec<_>>();
        Some(RouteResult {
            jumps: waypoints.len().saturating_sub(1),
            parsecs,
            waypoints,
        })
    }
}

/// Resolve a route endpoint to a world index. Accepts either a
/// `"Sector Name 0101"` reference (mirroring `RouteHandler.ResolveLocation`'s
/// "sector + 4-digit hex" parsing) **or** a bare world name (case-insensitive,
/// exact match). Returns the world's index in the [`WorldIndex`], or `None`.
pub fn resolve_location(index: &WorldIndex, query: &str) -> Option<usize> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    // 1) "<sector> <hex>" — split off a trailing 4-digit hex and match it.
    if let Some((sector, hex)) = q.rsplit_once(char::is_whitespace) {
        let sector = sector.trim();
        if let Some((col, row)) = parse_hex(hex.trim()) {
            if let Some(i) = index
                .worlds
                .iter()
                .position(|w| w.sector.eq_ignore_ascii_case(sector) && w.hex == hex_label(col, row))
            {
                return Some(i);
            }
        }
    }
    // 2) Bare world name (exact, case-insensitive) — "Regina", "Mora", …
    index
        .worlds
        .iter()
        .position(|w| w.name.eq_ignore_ascii_case(q))
}

/// Format a 1-based (col, row) back into a 4-digit hex label.
fn hex_label(col: i32, row: i32) -> String {
    format!("{col:02}{row:02}")
}

/// Code → full display name from `res/t5ss/allegiance_codes.tab` (columns
/// `Code Legacy BaseCode Name Location`) — e.g. `ImDd` → "Third Imperium,
/// Domain of Deneb". Used to label the printable route sheet.
fn load_allegiance_names(res_dir: &FsPath) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let path = res_dir.join("t5ss").join("allegiance_codes.tab");
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            if let (Some(code), Some(name)) = (cols.first(), cols.get(3)) {
                let (code, name) = (code.trim(), name.trim());
                if !code.is_empty() && !name.is_empty() {
                    map.insert(code.to_string(), name.to_string());
                }
            }
        }
    }
    map
}

/// Build the per-milieu world index by loading every sector's worlds.
pub fn build_world_index(res_dir: &FsPath, milieu: &str, universe: &Universe) -> WorldIndex {
    let dir = res_dir.join("Sectors").join(milieu);
    let alleg_names = load_allegiance_names(res_dir);
    let mut worlds: Vec<IndexedWorld> = Vec::new();

    for entry in &universe.sectors {
        let loc = entry.location;
        let Some((data_file, outcome)) = resolve_and_parse_worlds(&dir, &entry.name, Some(entry))
        else {
            continue;
        };
        // Subsector letter (A–P) → display name for this sector, so each world's
        // hex resolves to its `Subsector` name (reference `World.SubsectorName`).
        let meta_xml = crate::read_meta_xml(&dir, &data_file, entry);
        let subsector_names: HashMap<String, String> = sector_subsectors(&meta_xml)
            .into_iter()
            .filter(|s| !s.name.is_empty())
            .map(|s| (s.index, s.name))
            .collect();
        for w in outcome.worlds {
            let Some((col, row)) = parse_hex(&w.hex) else {
                continue;
            };
            let coord = Coord::new(loc.x * 32 + col, loc.y * 40 + row);
            let node = RouteWorld {
                coord,
                red: w.zone == "R",
                anomaly: is_anomaly(&w.uwp),
                imperial: is_default_imperial(&w.allegiance),
                refuel: has_refuel(&w.uwp, &w.pbg),
            };
            let allegiance = alleg_names
                .get(&w.allegiance)
                .cloned()
                .unwrap_or(w.allegiance);
            let subsector = subsector_names
                .get(&subsector_letter(&w.hex).to_string())
                .cloned()
                .unwrap_or_default();
            worlds.push(IndexedWorld {
                node,
                name: w.name,
                hex: w.hex,
                sector: entry.name.clone(),
                subsector,
                uwp: w.uwp,
                pbg: w.pbg,
                zone: w.zone,
                allegiance,
            });
        }
    }

    // Dedup by coord (a hex can only hold one main world; first wins) and build
    // the parallel node slice + coord lookup.
    let mut by_coord: HashMap<Coord, usize> = HashMap::new();
    let mut nodes: Vec<RouteWorld> = Vec::with_capacity(worlds.len());
    let mut deduped: Vec<IndexedWorld> = Vec::with_capacity(worlds.len());
    for w in worlds {
        if by_coord.contains_key(&w.node.coord) {
            continue;
        }
        by_coord.insert(w.node.coord, deduped.len());
        nodes.push(w.node.clone());
        deduped.push(w);
    }

    WorldIndex {
        worlds: deduped,
        nodes,
    }
}

/// An "anomaly" / deep-space object has no normal UWP (the reference's
/// `World.IsAnomaly`): a starport of `X`/`?` and an unset profile, conventionally
/// written with `?` placeholders. Treat a UWP starting with `?` as an anomaly.
fn is_anomaly(uwp: &str) -> bool {
    uwp.starts_with('?') || uwp.is_empty()
}

/// Is the allegiance the default Imperial allegiance? The default codes all
/// start with `"Im"` (e.g. `ImDd`, `ImLc`, …); approximate `IsDefaultAllegiance`
/// with that prefix.
fn is_default_imperial(allegiance: &str) -> bool {
    allegiance.starts_with("Im")
}

/// Wilderness refuelling available: a gas giant in-system (PBG's `G` digit > 0)
/// or surface water (UWP hydrographics digit > 0). Mirrors `World.GasGiants`/
/// `World.WaterPresent` used by `RouteHandler`'s `wild` filter.
fn has_refuel(uwp: &str, pbg: &str) -> bool {
    let gas_giants = pbg.chars().nth(2).and_then(|c| c.to_digit(16)).unwrap_or(0) > 0;
    // UWP hydrographics is the 4th character (index 3): C430698-9 → '3'.
    let water = uwp.chars().nth(3).and_then(|c| c.to_digit(16)).unwrap_or(0) > 0;
    gas_giants || water
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuel_detection() {
        // Hydrographics 3, gas giants 0 → water present.
        assert!(has_refuel("C430698-9", "613"));
        // Hydrographics 0, gas giants 1 → gas giant present.
        assert!(has_refuel("C400698-9", "601"));
        // No water, no gas giants.
        assert!(!has_refuel("C400698-9", "600"));
    }

    #[test]
    fn anomaly_detection() {
        assert!(is_anomaly("???????-?"));
        assert!(!is_anomaly("C430698-9"));
    }

    #[test]
    fn hex_label_pads() {
        assert_eq!(hex_label(1, 1), "0101");
        assert_eq!(hex_label(32, 40), "3240");
    }
}
