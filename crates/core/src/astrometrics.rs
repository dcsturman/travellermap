//! Coordinate math for the Traveller hex grid.
//!
//! Ported (incrementally) from the reference `server/Astrometrics.cs`. A
//! sector is 32×40 parsecs; hexes use a "0xyy" four-digit label (column 01-32,
//! row 01-40). World coordinates are absolute parsec offsets from a fixed
//! reference, used for distance and rendering.

/// Sector width in parsecs (hex columns).
pub const SECTOR_WIDTH: i32 = 32;
/// Sector height in parsecs (hex rows).
pub const SECTOR_HEIGHT: i32 = 40;
/// Subsector dimensions in parsecs (a sector is 4×4 subsectors).
pub const SUBSECTOR_WIDTH: i32 = 8;
pub const SUBSECTOR_HEIGHT: i32 = 10;

/// Reference hex (`Astrometrics.ReferenceHex` = column 01, row 40). World-space
/// `(x, y)` is measured relative to it, so the Reference world sits at origin.
pub const REFERENCE_HEX_X: i32 = 1;
pub const REFERENCE_HEX_Y: i32 = 40;

/// `(sector_x, sector_y, hex_x, hex_y)` → world-space `(x, y)`.
///
/// Port of `Astrometrics.LocationToCoordinates`: absolute parsec offset from the
/// Reference world. Used by the `/api/coordinates` compatibility endpoint.
pub fn location_to_coordinates(sx: i32, sy: i32, hx: i32, hy: i32) -> (i32, i32) {
    let x = sx * SECTOR_WIDTH + (hx - REFERENCE_HEX_X);
    let y = sy * SECTOR_HEIGHT + (hy - REFERENCE_HEX_Y);
    (x, y)
}

/// World-space `(x, y)` → `(sector_x, sector_y, hex_x, hex_y)`.
///
/// Port of `Astrometrics.CoordinatesToLocation` (the inverse of
/// [`location_to_coordinates`]); the floor-division offsets handle negative
/// coordinates so a world always maps into a 1-based hex of the correct sector.
pub fn coordinates_to_location(x: i32, y: i32) -> (i32, i32, i32, i32) {
    let x = x + REFERENCE_HEX_X - 1;
    let y = y + REFERENCE_HEX_Y - 1;
    let sx = (x - if x < 0 { SECTOR_WIDTH - 1 } else { 0 }) / SECTOR_WIDTH;
    let sy = (y - if y < 0 { SECTOR_HEIGHT - 1 } else { 0 }) / SECTOR_HEIGHT;
    let hx = x - sx * SECTOR_WIDTH + 1;
    let hy = y - sy * SECTOR_HEIGHT + 1;
    (sx, sy, hx, hy)
}

/// Horizontal spacing between hex centers, `cos(pi/6)`.
pub const PARSEC_SCALE_X: f32 = 0.866_025_4;
/// Vertical spacing between hex centers.
pub const PARSEC_SCALE_Y: f32 = 1.0;

/// Absolute world coordinate in parsecs (not a screen pixel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Coord {
    pub x: i32,
    pub y: i32,
}

impl Coord {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Hex distance (in parsecs) on the offset-hex grid.
    ///
    /// Converts each offset coord (`x` = column, `y` = row) to cube coordinates
    /// using the **same** column stagger as the renderer — even columns shifted
    /// down ("even-q", matching [`crate`]'s `hex_parsec`) — then returns the cube
    /// distance, which is exact and symmetric.
    ///
    /// We deliberately do **not** port `Astrometrics.HexDistance` directly: that
    /// formula operates on the reference's *Reference-centric* absolute
    /// coordinates, whose column parity is **flipped** relative to ours
    /// (`Astrometrics.cs`: "even/odd column handling are opposite … since
    /// Reference is in an odd hex 0140"). Applying it to our column-parity coords
    /// undercounts column-crossing jumps by one in half the cases (e.g. it
    /// returned 2 for a true distance-3 pair). The cube form agrees with the
    /// reference's numbered `HexNeighbor` adjacency.
    pub fn hex_distance(self, other: Coord) -> i32 {
        // even-q offset → cube. `x + (x mod 2)` is always even, so `/ 2` is exact.
        let cube = |c: Coord| {
            let q = c.x;
            let r = c.y - (c.x + c.x.rem_euclid(2)) / 2;
            (q, r, -q - r)
        };
        let (aq, ar, az) = cube(self);
        let (bq, br, bz) = cube(other);
        ((aq - bq).abs() + (ar - br).abs() + (az - bz).abs()) / 2
    }
}

/// Hex distance (parsecs) between two **absolute, Reference-centric** map
/// coordinates — a direct port of `Astrometrics.HexDistance`. Use this (not
/// [`Coord::hex_distance`]) whenever the inputs are the reference's absolute
/// coordinates (e.g. `World.WorldX/WorldY`, the public-API parity); the cube
/// form in [`Coord::hex_distance`] is calibrated to our flipped column parity
/// and disagrees by one on half the column-crossing pairs. C# and Rust `%`
/// both truncate toward zero, so the parity test transfers verbatim.
pub fn reference_hex_distance(a: Coord, b: Coord) -> i32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let adx = dx.abs();
    let mut ody = dy + adx / 2;
    if a.x % 2 == 0 && b.x % 2 != 0 {
        ody += 1;
    }
    (adx - ody).max(ody.max(adx))
}

/// Subsector index `0..16` (A=0 … P=15) of a world's hex (`World.Subsector`):
/// `(col-1)/8 + (row-1)/10 * 4`. Out-of-range hex clamps to 0.
pub fn subsector_index(hex: &str) -> usize {
    let (c, r) = parse_hex(hex).unwrap_or((1, 1));
    let sc = ((c - 1) / SUBSECTOR_WIDTH).clamp(0, 3);
    let sr = ((r - 1) / SUBSECTOR_HEIGHT).clamp(0, 3);
    (sr * 4 + sc) as usize
}

/// Subsector letter `'A'..='P'` of a world's hex (`World.SS`).
pub fn subsector_letter(hex: &str) -> char {
    (b'A' + subsector_index(hex) as u8) as char
}

/// Quadrant index of a world's hex (`World.Quadrant`):
/// `(col-1)/16 + (row-1)/20 * 4` (ported verbatim from the reference).
pub fn quadrant_index(hex: &str) -> usize {
    let (c, r) = parse_hex(hex).unwrap_or((1, 1));
    let qc = (c - 1) / (SUBSECTOR_WIDTH * 2);
    let qr = (r - 1) / (SUBSECTOR_HEIGHT * 2);
    (qc + qr * 4).max(0) as usize
}

/// A world's hex relative to its subsector origin (`Hex.ToSubsectorString`),
/// e.g. `"0911"` → `"0101"`. Used when `sscoords=1`.
pub fn subsector_hex(hex: &str) -> String {
    let (c, r) = parse_hex(hex).unwrap_or((1, 1));
    let sc = (c - 1).rem_euclid(SUBSECTOR_WIDTH) + 1;
    let sr = (r - 1).rem_euclid(SUBSECTOR_HEIGHT) + 1;
    format!("{sc:02}{sr:02}")
}

/// Parse a four-digit hex label like `"1910"` into 1-based `(col, row)`
/// (column 01-32, row 01-40). Returns `None` if malformed.
pub fn parse_hex(hex: &str) -> Option<(i32, i32)> {
    if hex.len() != 4 || !hex.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let col = hex[0..2].parse().ok()?;
    let row = hex[2..4].parse().ok()?;
    Some((col, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_hex_distance_matches_csharp() {
        // Regina (Spinward Marches 1910) is absolute (-110,-70); Forboldn (1808)
        // is (-111,-72). Astrometrics.HexDistance gives 2 (a jump-2 neighbour) —
        // the column-parity correction is the whole point.
        let regina = Coord::new(-110, -70);
        let forboldn = Coord::new(-111, -72);
        assert_eq!(reference_hex_distance(regina, forboldn), 2);
        assert_eq!(reference_hex_distance(forboldn, regina), 2);
        // Zero distance to self, and a same-column step is exactly the row delta.
        assert_eq!(reference_hex_distance(regina, regina), 0);
        assert_eq!(reference_hex_distance(regina, Coord::new(-110, -68)), 2);
    }

    #[test]
    fn parses_hex_labels() {
        assert_eq!(parse_hex("1910"), Some((19, 10)));
        assert_eq!(parse_hex("0101"), Some((1, 1)));
        assert_eq!(parse_hex("3240"), Some((32, 40)));
        assert_eq!(parse_hex("19x0"), None);
        assert_eq!(parse_hex("191"), None);
    }

    #[test]
    fn distance_to_self_is_zero() {
        let a = Coord::new(5, 5);
        assert_eq!(a.hex_distance(a), 0);
    }

    #[test]
    fn adjacent_hexes_are_one_apart() {
        assert_eq!(Coord::new(1, 1).hex_distance(Coord::new(2, 1)), 1);
    }

    #[test]
    fn coordinates_match_reference() {
        // Spinward Marches sits at grid (-4,-1); Regina is hex 1910. The live
        // `/api/coordinates?sector=Spinward Marches&hex=1910` returns these.
        assert_eq!(location_to_coordinates(-4, -1, 19, 10), (-110, -70));
        // Round-trips back to the same sector/hex.
        assert_eq!(coordinates_to_location(-110, -70), (-4, -1, 19, 10));
        // Reference world itself is the world-space origin.
        assert_eq!(location_to_coordinates(0, 0, 1, 40), (0, 0));
        assert_eq!(coordinates_to_location(0, 0), (0, 0, 1, 40));
    }

    #[test]
    fn column_crossing_distance_is_correct() {
        // Theev (col 21) → Noricum (col 20), two rows down: 3 hops on the map
        // (down-left then down twice), not 2. The old reference-parity port
        // undercounted this to 2, letting a J-3 leg into a J-2 route.
        assert_eq!(Coord::new(21, 16).hex_distance(Coord::new(20, 18)), 3);
        // Symmetric.
        assert_eq!(Coord::new(20, 18).hex_distance(Coord::new(21, 16)), 3);
        // Same column is just the row delta.
        assert_eq!(Coord::new(20, 18).hex_distance(Coord::new(20, 20)), 2);
        // The six immediate neighbours of an odd column are all distance 1.
        for n in [(20, 16), (20, 15), (21, 15), (22, 15), (22, 16), (21, 17)] {
            assert_eq!(
                Coord::new(21, 16).hex_distance(Coord::new(n.0, n.1)),
                1,
                "{n:?}"
            );
        }
    }
}
