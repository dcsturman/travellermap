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
    /// Mirrors `Astrometrics.HexDistance`; the `ody` correction accounts for
    /// odd/even column staggering.
    pub fn hex_distance(self, other: Coord) -> i32 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        let adx = dx.abs();
        let mut ody = dy + adx / 2;
        if self.x % 2 == 0 && other.x % 2 != 0 {
            ody += 1;
        }
        (adx - ody).max(ody.max(adx))
    }
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
}
