//! Wire types streamed from `tmap-backend` to the `tmap-frontend` client.
//!
//! These are the contract: the backend serializes them, the client
//! deserializes and renders them. Reimplements the payloads the reference
//! `server/api/` handlers produce, but as the only thing the backend does (no
//! server-side image rendering).
//!
//! **Full fidelity:** `World` carries every field of a T5 Second Survey row so
//! the parser is never lossy. Lower LOD tiers are *projections* over these
//! types (a subset of fields/features), never a different parse — see the LOD
//! invariant in `PORT_PLAN.md`.

use crate::astrometrics::Coord;
use serde::{Deserialize, Serialize};

/// A single world (system main world) within a sector — one T5 `.tab` row.
/// Empty/absent fields are omitted from the wire form to keep payloads lean.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct World {
    /// Four-digit hex label within the sector, e.g. `"0101"` (col/row, 01-32 / 01-40).
    pub hex: String,
    pub name: String,
    /// Universal World Profile, e.g. `"C430698-9"`.
    pub uwp: String,
    /// Base codes, e.g. `"NS"`. Empty when none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bases: String,
    /// Raw trade-code / remarks string, space-separated tokens (e.g. `"De Na Ni Po"`).
    /// Kept raw for fidelity; use [`World::codes`] to split.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remarks: String,
    /// Travel zone: `"A"` (Amber), `"R"` (Red), or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// Population/Belts/Gas-giants digits, e.g. `"613"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pbg: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allegiance: String,
    /// Stellar data, e.g. `"K9 V"` (the `Stars`/`Stellar` column).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stellar: String,
    /// Importance extension `{Ix}`, raw e.g. `"{ -1 }"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<String>,
    /// Economic extension `(Ex)`, raw e.g. `"(C53-1)"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub economic: Option<String>,
    /// Cultural extension `[Cx]`, raw e.g. `"[6559]"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cultural: Option<String>,
    /// Nobility codes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nobility: Option<String>,
    /// Worlds-in-system count (`W` column).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worlds: Option<u8>,
    /// Resource Units (`RU` column).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_units: Option<i32>,
}

impl World {
    /// Trade/remark codes as individual whitespace-separated tokens.
    pub fn codes(&self) -> impl Iterator<Item = &str> {
        self.remarks.split_whitespace()
    }
}

/// One named subsector within a sector. `index` is the letter `A`–`P` (a 4×4
/// grid, reading order), which fixes its position; each spans 8×10 parsecs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subsector {
    pub index: String,
    pub name: String,
}

/// Sector identity and where it sits on the map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorInfo {
    pub name: String,
    /// Sector grid position (e.g. Spinward Marches is `(-4, -1)`). Not present
    /// in `.tab` data — populated from the sector index/metadata (Phase 4/7),
    /// so `None` until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Coord>,
    /// Milieu key, e.g. `"M1105"`.
    pub milieu: String,
    /// Named subsectors (from the sector metadata `.xml`); empty if unavailable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsectors: Vec<Subsector>,
    /// Space-separated review tags from the `<Sector Tags="…">` attribute:
    /// `Official Preserve InReview Unreviewed Apocryphal`. Empty if absent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tags: String,
    /// Raw `<Credits>` text (may be HTML-encoded). `None` if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<String>,
}

/// A per-sector (micro) allegiance border: the closed loop of boundary hexes
/// plus an optional label. (The exact hex-edge path is computed at render time;
/// pixel-perfect `BorderPath` geometry is a later refinement.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub allegiance: String,
    /// Boundary hexes (4-digit labels) forming the perimeter loop. Not sent to
    /// the client (it renders from `region`); kept for region computation.
    #[serde(default, skip_serializing)]
    pub hexes: Vec<String>,
    /// All **absolute** world hexes inside the border (boundary + interior),
    /// computed by the backend. The client fills these and strokes the edges
    /// where a region hex borders a non-region hex (exact hex-edge border).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region: Vec<(i32, i32)>,
    /// Explicit per-border color (`Color` attr), e.g. `"#00FF80"`. Takes
    /// precedence over the allegiance color lookup (this is how pocket empires
    /// get their distinct colors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_position: Option<String>,
    /// Wrap the label at whitespace-not-before-lowercase (reference `WrapLabel`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrap_label: bool,
    /// Label nudge in hex units (applied as `×0.7` parsec), reference
    /// `LabelOffsetX`/`LabelOffsetY`.
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub label_offset: (f32, f32),
}

/// A hand-placed standalone region/area label from sector metadata
/// (`<Label Hex= Color= Size= Wrap=>text</Label>`), e.g. "Outrim Void".
/// Distinct from a border label — it has no allegiance region, just a position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorLabel {
    pub text: String,
    /// 4-digit hex within the sector.
    pub hex: String,
    /// Explicit color (`Color` attr); `None` → the default amber.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// `"small"` | `"large"` | `None` (medium) — picks the font size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrap: bool,
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub offset: (f32, f32),
}

/// Serde skip helper: a `(0.0, 0.0)` offset is the common case.
fn is_zero2(v: &(f32, f32)) -> bool {
    v.0 == 0.0 && v.1 == 0.0
}

/// A trade/communication route segment between two hexes. Offsets are in
/// **sectors** (cross-sector routes reference a neighbor sector).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub start_offset: (i32, i32),
    #[serde(default)]
    pub end_offset: (i32, i32),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allegiance: Option<String>,
    /// Explicit route color from the `<Route Color="...">` attribute, if any;
    /// otherwise the renderer uses the default gray.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A sector's worlds (and borders/routes), streamed as one payload to the
/// client for rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorData {
    pub info: SectorInfo,
    pub worlds: Vec<World>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub borders: Vec<Border>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    /// Hand-placed standalone labels (`<Label>`), e.g. "Outrim Void".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<SectorLabel>,
}

/// One entry in the universe index: a sector's identity and grid position,
/// without its worlds. The client uses this to map viewport → which sectors to
/// fetch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorIndexEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abbreviation: Option<String>,
    /// Sector grid position (Core at `(0,0)`, +x trailing, +y rimward).
    pub location: Coord,
    /// All `<Name>` entries (canonical first, then localized `Lang=…`). Source
    /// for the public `/api/universe` `Names` array; backend-internal otherwise.
    #[serde(default, skip_serializing)]
    pub names: Vec<SectorName>,
    /// The sector's own review tags (`<Sector Tags="…">`, e.g. `"Official"`),
    /// before the milieu metafile tag is appended. Backend-internal.
    #[serde(default, skip_serializing)]
    pub tags: String,
    /// Data filename (e.g. `"Spinward Marches.tab"`) — backend-only, from the
    /// milieu region list; the client doesn't need it.
    #[serde(default, skip_serializing)]
    pub data_file: Option<String>,
    /// Data format: `"TabDelimited"`, `"SecondSurvey"`, or `"SEC"` (backend-only).
    #[serde(default, skip_serializing)]
    pub data_format: Option<String>,
    /// Metadata filename (e.g. `"Beyond.xml"` for sector "The Beyond") —
    /// backend-only. The name often differs from the sector's display name.
    #[serde(default, skip_serializing)]
    pub metadata_file: Option<String>,
}

/// The set of sectors in a milieu and where they sit — the navigation index.
/// In-memory only; the wire shape is [`UniverseResult`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Universe {
    pub milieu: String,
    pub sectors: Vec<SectorIndexEntry>,
}

/// Provenance attributes from a sector's `<DataFile>` element (region list),
/// surfaced in the `# …` metadata comment block of SEC/SecondSurvey output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DataFileMeta {
    pub author: Option<String>,
    pub publisher: Option<String>,
    pub copyright: Option<String>,
    pub source: Option<String>,
    pub reference: Option<String>,
}

/// One localized sector name, matching the public-API `Names` element
/// (`{"Text","Lang"}`). `lang` is absent for the canonical (English) name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorName {
    #[serde(rename = "Text")]
    pub text: String,
    #[serde(rename = "Lang", default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// One sector in the public `/api/universe` response (port of
/// `UniverseHandler`'s `SectorResult`). PascalCase, flat `X`/`Y`, and a `Names`
/// array — the documented shape, consumed by our own Leptos client too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UniverseSector {
    #[serde(rename = "X")]
    pub x: i32,
    #[serde(rename = "Y")]
    pub y: i32,
    #[serde(rename = "Milieu")]
    pub milieu: String,
    #[serde(rename = "Abbreviation", default, skip_serializing_if = "Option::is_none")]
    pub abbreviation: Option<String>,
    #[serde(rename = "Tags", default, skip_serializing_if = "String::is_empty")]
    pub tags: String,
    #[serde(rename = "Names")]
    pub names: Vec<SectorName>,
}

/// The public `/api/universe` envelope: `{"Sectors":[…]}` (`UniverseResult`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UniverseResult {
    #[serde(rename = "Sectors")]
    pub sectors: Vec<UniverseSector>,
}

/// One connected sub-path of a [`VectorObject`]. A border/rift can comprise
/// several disjoint regions (islands), each its own sub-path; drawing them as
/// one polyline would connect them with spurious straight lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub points: Vec<(f32, f32)>,
    /// Whether the last point joins back to the first (closed region vs. open route).
    pub closed: bool,
}

/// A macro-scale overlay (a polity border, trade route, or rift) from
/// `res/Vectors/`. Points are in the file's own space; the world-parsec
/// position of a point is `(p − origin) · scale` (see the reference
/// `VectorObject`), then the usual `PARSEC_SCALE_X` horizontal compression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorObject {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_options: Option<String>,
    pub origin: (f32, f32),
    pub scale: (f32, f32),
    /// Disjoint sub-paths (split per the file's GDI+ point-type bytes).
    pub paths: Vec<SubPath>,
    /// World-space anchor for the region's name label (uncompressed; the client
    /// applies `PARSEC_SCALE_X`). Computed from `Bounds` + `NameX/NameY`,
    /// mirroring the reference `VectorObject.NamePosition`. `None` if no bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<(f32, f32)>,
}

/// A macro-scale "important world" marker (capital / homeworld) from
/// `res/labels/Worlds.xml` — a dot at `coord` with a name label offset by
/// `bias` (the reference `WorldObject` LabelBiasX/Y: −1/0/+1 per axis).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldLabel {
    pub name: String,
    /// Absolute world hex (col, row), `sector·32 + col`, `sector·40 + row`.
    pub coord: Coord,
    pub bias: (i8, i8),
}

/// A free-placed map label from the `Text\tX\tY\tMinor` label files
/// (`res/labels/mega_labels.tab` galaxy-scale names, `minor_labels.tab` region
/// names). `x`/`y` are in the reference map's (x-compressed) world coordinates;
/// `text` may contain `\n` line breaks. The `minor` flag selects the smaller
/// font/alternate color — its meaning differs per dataset (see the draw passes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapLabel {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub minor: bool,
}

/// The macro overlays shown when zoomed out, grouped by kind. Charted-space
/// scale and milieu-independent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overlays {
    pub borders: Vec<VectorObject>,
    pub routes: Vec<VectorObject>,
    pub rifts: Vec<VectorObject>,
    /// Capitals + homeworlds (Worlds.xml). `#[serde(default)]` so older cached
    /// payloads without this field still deserialize.
    #[serde(default)]
    pub labels: Vec<WorldLabel>,
    /// Galaxy-scale labels (mega_labels.tab), shown at the most zoomed-out view.
    #[serde(default)]
    pub mega_labels: Vec<MapLabel>,
    /// Minor region labels (minor_labels.tab) — red region names ("Mixed Client
    /// States", …) drawn over the macro view (scale 0.5–4).
    #[serde(default)]
    pub minor_labels: Vec<MapLabel>,
}

/// One search hit — a world or sector — with where to jump to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub name: String,
    /// `"world"` or `"sector"`.
    pub kind: String,
    pub sector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// Absolute world hex (col, row) to center the view on.
    pub coord: Coord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub results: Vec<SearchResult>,
}

/// A jump-route request (mirrors the reference `RouteHandler` query params).
/// `start`/`end` are `"Sector Name 0101"` strings the backend resolves to
/// worlds; the algorithm itself works on resolved [`Coord`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRequest {
    pub start: String,
    pub end: String,
    /// Jump range in parsecs (the ship's jump drive rating). Clamped 1..=12.
    pub jump: i32,
    pub milieu: String,
}

/// One stop along a computed jump route — a world the route passes through.
/// Mirrors the reference `RouteStop` payload (name + location), plus the
/// absolute [`Coord`] the client needs to draw the polyline directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteWaypoint {
    pub name: String,
    /// Four-digit hex label within its sector, e.g. `"0101"`.
    pub hex: String,
    /// Absolute world coordinate (parsec offsets), for client-side drawing.
    pub coord: Coord,
    /// Sector display name the world belongs to.
    pub sector: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uwp: String,
    /// Population/Belts/Gas-giants digits; the third digit is the gas-giant count.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pbg: String,
    /// Travel zone (`"A"`/`"R"`/empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// Full allegiance display name (e.g. "Third Imperium, Domain of Deneb"),
    /// resolved from the world's allegiance code — for the printable route sheet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allegiance: String,
}

/// The result of a jump-route search: an ordered list of waypoints from start
/// to end, plus aggregate stats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteResult {
    pub waypoints: Vec<RouteWaypoint>,
    /// Number of jumps taken (= `waypoints.len() - 1`).
    pub jumps: usize,
    /// Total path length in parsecs (sum of per-jump hex distances).
    pub parsecs: i32,
}
