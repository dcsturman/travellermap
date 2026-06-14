//! Callisto (non-reference, dev-only): generate a solar-system PNG for a world
//! using the `worldgen` crate. Compiled only under `--features callisto`.
//!
//! The image is deterministic: the seed comes from the world's identity (sector
//! name + hex) and the system shape is constrained by what we already know from
//! the world's T5 data — its stellar roster, PBG (belts/gas-giants), worlds
//! count, and main-world UWP. See worldgen/docs/library-integration.md.

use tmap_core::dto::World;
use tmap_core::world_util::split_pbg;
use worldgen::{
    build_constraints, generate_system_png_scaled, seed::system_seed, StarSize, StarSpec, StarType,
};

use crate::AppState;

/// Supersample factor for the system render. 2× (3200×1800) keeps body labels
/// crisp when the client zooms into the image.
const RENDER_SCALE: f32 = 2.0;

/// Find a world by `(milieu, sector name, hex)`, loading + parsing the sector's
/// data the same way `build_sector_bytes` does (shared `resolve_and_parse_worlds`).
pub(crate) fn lookup_world(
    state: &AppState,
    milieu: &str,
    sector: &str,
    hex: &str,
) -> Option<World> {
    let universe = state.universe(milieu).ok()?;
    let entry = universe.sectors.iter().find(|s| s.name == sector)?;
    let dir = state.res_dir.join("Sectors").join(milieu);
    let (_data_file, outcome) = crate::resolve_and_parse_worlds(&dir, sector, Some(entry))?;
    outcome.worlds.into_iter().find(|w| w.hex == hex)
}

/// Generate the system PNG bytes for a world. `Err` carries a short,
/// user-facing reason (e.g. a partial/malformed main-world UWP that worldgen
/// rejects) suitable for an HTTP 422 body.
pub(crate) fn world_to_png(sector: &str, world: &World) -> Result<Vec<u8>, String> {
    // Seed from identity: sector name + the two hex digit-pairs.
    let (hx, hy) = parse_hex_pair(&world.hex)
        .ok_or_else(|| format!("bad hex '{}'", world.hex))?;
    let seed = system_seed(sector, hx, hy);

    // Stars from the stellar roster; counts from PBG + worlds.
    let stars = parse_stellar(&world.stellar);
    let pbg = split_pbg(&world.pbg);
    let belts = pbg.belts.unwrap_or(0).max(0) as usize;
    let gas_giants = pbg.gas_giants.unwrap_or(0).max(0) as usize;
    // Additional rocky planets beyond the main world: back out the main world,
    // belts, and gas giants from the system's worlds count (clamped ≥ 0). When
    // the count is unknown, ask for none (worldgen still places the rest).
    let planets = world
        .worlds
        .map(|w| (w as i32 - 1 - belts as i32 - gas_giants as i32).max(0) as usize)
        .unwrap_or(0);

    let constraints = build_constraints(&world.name, &world.uwp, &stars, gas_giants, belts, planets)
        .map_err(|e| e.to_string())?;
    generate_system_png_scaled(seed, constraints, RENDER_SCALE).map_err(|e| e.to_string())
}

/// `"0101"` → `(1, 1)` (column, row as `u8`). `None` if not 4 digits.
fn parse_hex_pair(hex: &str) -> Option<(u8, u8)> {
    if hex.len() != 4 || !hex.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((hex[0..2].parse().ok()?, hex[2..4].parse().ok()?))
}

/// Parse a Traveller-Map "Stellar" string like `"G2 V M9 V M6 V"` into typed
/// `StarSpec`s. Ported verbatim from worldgen's private `parse_stellar`
/// (worldgen/src/components/system_generator.rs): tolerant — skips brown
/// dwarfs (`BD`) and any token not starting with a known spectral letter, and
/// accepts the size inline (`"G2V"`) or as a following token (`"G2 V"`).
fn parse_stellar(s: &str) -> Vec<StarSpec> {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok.eq_ignore_ascii_case("BD") {
            continue;
        }
        let bytes = tok.as_bytes();
        if bytes.is_empty() {
            continue;
        }
        let spectral = match bytes[0] {
            b'O' => StarType::O,
            b'B' => StarType::B,
            b'A' => StarType::A,
            b'F' => StarType::F,
            b'G' => StarType::G,
            b'K' => StarType::K,
            b'M' => StarType::M,
            _ => continue,
        };
        let mut subtype: Option<u8> = None;
        let mut j = 1;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            subtype = Some(subtype.unwrap_or(0) * 10 + (bytes[j] - b'0'));
            j += 1;
        }
        let size_str: String = if j < bytes.len() {
            std::str::from_utf8(&bytes[j..]).unwrap_or("").to_string()
        } else if i < tokens.len() && is_size_token(tokens[i]) {
            let s = tokens[i].to_string();
            i += 1;
            s
        } else {
            continue;
        };
        let size = match size_str.as_str() {
            "Ia" => StarSize::Ia,
            "Ib" => StarSize::Ib,
            "II" => StarSize::II,
            "III" => StarSize::III,
            "IV" => StarSize::IV,
            "V" => StarSize::V,
            "VI" => StarSize::VI,
            "D" => StarSize::D,
            _ => continue,
        };
        out.push(match subtype {
            Some(st) => StarSpec::new(spectral, st, size),
            None => StarSpec::with_rolled_subtype(spectral, size),
        });
    }
    out
}

fn is_size_token(s: &str) -> bool {
    matches!(s, "Ia" | "Ib" | "II" | "III" | "IV" | "V" | "VI" | "D")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(hex: &str, name: &str, uwp: &str, pbg: &str, stellar: &str, worlds: Option<u8>) -> World {
        World {
            hex: hex.into(),
            name: name.into(),
            uwp: uwp.into(),
            pbg: pbg.into(),
            stellar: stellar.into(),
            worlds,
            ..Default::default()
        }
    }

    #[test]
    fn parse_stellar_multi_and_inline() {
        assert_eq!(parse_stellar("G2 V M9 V M6 V").len(), 3);
        assert_eq!(parse_stellar("M0 V").len(), 1);
        // Inline size, brown-dwarf skipped, junk skipped.
        assert_eq!(parse_stellar("G2V BD K5 V").len(), 2);
        assert_eq!(parse_stellar("").len(), 0);
    }

    #[test]
    fn hex_pair() {
        assert_eq!(parse_hex_pair("3128"), Some((31, 28)));
        assert_eq!(parse_hex_pair("0101"), Some((1, 1)));
        assert_eq!(parse_hex_pair("31"), None);
        assert_eq!(parse_hex_pair("31X8"), None);
    }

    #[test]
    fn noricum_renders_png() {
        // Trojan Reach 3128 with its real three-star roster.
        let w = world("3128", "Noricum", "D8867BB-1", "503", "G2 V M9 V M6 V", Some(8));
        let png = world_to_png("Trojan Reach", &w).expect("should generate");
        assert!(png.len() > 8 && &png[1..4] == b"PNG", "expected PNG magic, got {} bytes", png.len());
    }

    #[test]
    fn partial_uwp_errors() {
        let w = world("0101", "Mystery", "X???????-?", "000", "M0 V", None);
        assert!(world_to_png("Spinward Marches", &w).is_err());
    }
}
