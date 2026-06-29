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

use std::fmt::Write as _;

use crate::astrometrics::Coord;
use serde::{Deserialize, Serialize};

/// Escape the five XML metacharacters (`& < > " '`) for element text or a
/// double-quoted attribute value. Hand-rolled so the `to_xml` serializers below
/// can build the reference XML shapes without an XML-serde dependency (keeps
/// `tmap-core` lean and wasm-friendly).
pub(crate) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// `<Tag>escaped-text</Tag>` (text-content element).
pub(crate) fn xml_el(out: &mut String, tag: &str, text: &str) {
    out.push('<');
    out.push_str(tag);
    out.push('>');
    out.push_str(&xml_escape(text));
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

/// `<Tag>text</Tag>` only when `text` is `Some` (a null field is dropped, like
/// the reference XML serializer skips null members).
fn xml_el_opt(out: &mut String, tag: &str, text: &Option<String>) {
    if let Some(t) = text {
        xml_el(out, tag, t);
    }
}

/// ` name="escaped-value"` (attribute fragment, leading space included).
pub(crate) fn xml_attr(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(&xml_escape(value));
    out.push('"');
}

/// A non-null string element, emitting `<Tag />` when empty (the .NET
/// `XmlSerializer` writes an empty-but-present string member as a self-closing
/// element, e.g. `<Bases />`).
fn xml_el_str(out: &mut String, tag: &str, text: &str) {
    if text.is_empty() {
        out.push('<');
        out.push_str(tag);
        out.push_str(" />");
    } else {
        xml_el(out, tag, text);
    }
}

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

/// The public per-world result shape used by `/api/jumpworlds` (and `/api/search`
/// world hits) — port of the reference `World`'s serialized members. PascalCase,
/// with the computed/denormalized fields (`SS`, `Subsector`, `Quadrant`,
/// `WorldX/Y`, `LegacyBaseCode`, `Sector`, `SubsectorName`,
/// `SectorAbbreviation`, `AllegianceName`) the reference adds on serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldResult {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Hex")]
    pub hex: String,
    #[serde(rename = "UWP")]
    pub uwp: String,
    #[serde(rename = "PBG")]
    pub pbg: String,
    #[serde(rename = "Zone")]
    pub zone: String,
    #[serde(rename = "Bases")]
    pub bases: String,
    #[serde(rename = "Allegiance")]
    pub allegiance: String,
    #[serde(rename = "Stellar")]
    pub stellar: String,
    #[serde(rename = "SS")]
    pub ss: String,
    #[serde(rename = "Ix")]
    pub ix: Option<String>,
    #[serde(rename = "Ex")]
    pub ex: Option<String>,
    #[serde(rename = "Cx")]
    pub cx: Option<String>,
    #[serde(rename = "Nobility")]
    pub nobility: String,
    #[serde(rename = "Worlds")]
    pub worlds: i32,
    #[serde(rename = "ResourceUnits")]
    pub resource_units: Option<i32>,
    #[serde(rename = "Subsector")]
    pub subsector: i32,
    #[serde(rename = "Quadrant")]
    pub quadrant: i32,
    #[serde(rename = "WorldX")]
    pub world_x: i32,
    #[serde(rename = "WorldY")]
    pub world_y: i32,
    #[serde(rename = "Remarks")]
    pub remarks: String,
    #[serde(rename = "LegacyBaseCode")]
    pub legacy_base_code: String,
    #[serde(rename = "Sector")]
    pub sector: String,
    #[serde(rename = "SubsectorName")]
    pub subsector_name: String,
    #[serde(rename = "SectorAbbreviation")]
    pub sector_abbreviation: Option<String>,
    #[serde(rename = "AllegianceName")]
    pub allegiance_name: String,
}

/// The `/api/jumpworlds` envelope: `{"Worlds":[…]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JumpWorldsResult {
    #[serde(rename = "Worlds")]
    pub worlds: Vec<WorldResult>,
}

impl JumpWorldsResult {
    /// `<JumpWorlds><World>…</World>…</JumpWorlds>` — the reference
    /// `JumpWorldsResult` XML (`JumpWorldsHandler.cs`), which serializes the
    /// `World` domain type. The .NET `XmlSerializer` emits only **settable**
    /// members (the computed getters `SS`/`Subsector`/`Quadrant`/`WorldX/Y`/
    /// `Sector`/`SubsectorName`/`SectorAbbreviation`/`AllegianceName` are
    /// read-only, so they are absent from XML though present in JSON), in
    /// declaration order: Name, Hex, UWP, PBG, Zone, Bases, Allegiance, Stellar,
    /// Ix, Ex, Cx, Nobility, Worlds, ResourceUnits, Remarks, LegacyBaseCode.
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\"?>\n<JumpWorlds>");
        for w in &self.worlds {
            out.push_str("<World>");
            xml_el(&mut out, "Name", &w.name);
            xml_el(&mut out, "Hex", &w.hex);
            xml_el(&mut out, "UWP", &w.uwp);
            xml_el(&mut out, "PBG", &w.pbg);
            xml_el_str(&mut out, "Zone", &w.zone);
            xml_el_str(&mut out, "Bases", &w.bases);
            xml_el(&mut out, "Allegiance", &w.allegiance);
            xml_el_str(&mut out, "Stellar", &w.stellar);
            xml_el_opt(&mut out, "Ix", &w.ix);
            xml_el_opt(&mut out, "Ex", &w.ex);
            xml_el_opt(&mut out, "Cx", &w.cx);
            xml_el_str(&mut out, "Nobility", &w.nobility);
            xml_el(&mut out, "Worlds", &w.worlds.to_string());
            if let Some(ru) = w.resource_units {
                xml_el(&mut out, "ResourceUnits", &ru.to_string());
            }
            xml_el_str(&mut out, "Remarks", &w.remarks);
            xml_el_str(&mut out, "LegacyBaseCode", &w.legacy_base_code);
            out.push_str("</World>");
        }
        out.push_str("</JumpWorlds>");
        out
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
    /// The sector's `Milieu` attribute (or its `DataFile`'s), if any — the
    /// reference's `CanonicalMilieu` before defaulting to `M1105`. `None` means
    /// "belongs to the default milieu". Backend-only.
    #[serde(default, skip_serializing)]
    pub milieu: Option<String>,
    /// Tag carried by the `milieu.tab` metafile this sector was loaded from
    /// (e.g. `"OTU"`, `"Faraway"`) — appended to the sector's own `tags` in the
    /// universe response. Set during aggregation; backend-only.
    #[serde(default, skip_serializing)]
    pub metafile_tag: Option<String>,
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
    pub title: Option<String>,
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
    #[serde(rename = "Source", default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
    #[serde(
        rename = "Abbreviation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
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

impl UniverseResult {
    /// `<Universe><Sector Abbreviation Tags><X/><Y/><Milieu/><Name/>…` — the
    /// reference `UniverseResult` XML (`UniverseHandler.cs`). `X`/`Y`/`Milieu`/
    /// `Name` are child elements; `Abbreviation`/`Tags` are attributes (dropped
    /// when empty, matching the .NET `[XmlAttribute]` null/empty handling).
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\"?>\n<Universe>");
        for s in &self.sectors {
            out.push_str("<Sector");
            if let Some(abbr) = &s.abbreviation {
                write!(out, " Abbreviation=\"{}\"", xml_escape(abbr)).unwrap();
            }
            if !s.tags.is_empty() {
                write!(out, " Tags=\"{}\"", xml_escape(&s.tags)).unwrap();
            }
            out.push('>');
            xml_el(&mut out, "X", &s.x.to_string());
            xml_el(&mut out, "Y", &s.y.to_string());
            xml_el(&mut out, "Milieu", &s.milieu);
            for n in &s.names {
                match &n.lang {
                    Some(lang) => write!(out, "<Name Lang=\"{}\">", xml_escape(lang)).unwrap(),
                    None => out.push_str("<Name>"),
                }
                out.push_str(&xml_escape(&n.text));
                out.push_str("</Name>");
            }
            out.push_str("</Sector>");
        }
        out.push_str("</Universe>");
        out
    }
}

/// The public `/api/credits` response (port of `CreditsHandler`'s
/// `CreditsResult`). PascalCase; every string field is omitted when absent
/// (the reference's JSON serializer drops nulls). `SectorX`/`SectorY` are
/// always present.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditsResult {
    #[serde(rename = "Credits", default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<String>,

    #[serde(rename = "SectorX")]
    pub sector_x: i32,
    #[serde(rename = "SectorY")]
    pub sector_y: i32,
    #[serde(
        rename = "SectorName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_name: Option<String>,
    #[serde(
        rename = "SectorAuthor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_author: Option<String>,
    #[serde(
        rename = "SectorSource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_source: Option<String>,
    #[serde(
        rename = "SectorPublisher",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_publisher: Option<String>,
    #[serde(
        rename = "SectorCopyright",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_copyright: Option<String>,
    #[serde(rename = "SectorRef", default, skip_serializing_if = "Option::is_none")]
    pub sector_ref: Option<String>,
    #[serde(
        rename = "SectorMilieu",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_milieu: Option<String>,
    #[serde(
        rename = "SectorTags",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sector_tags: Option<String>,

    #[serde(
        rename = "RouteCredits",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub route_credits: Option<String>,

    #[serde(
        rename = "SubsectorName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub subsector_name: Option<String>,
    #[serde(
        rename = "SubsectorIndex",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub subsector_index: Option<String>,
    #[serde(
        rename = "SubsectorCredits",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub subsector_credits: Option<String>,

    #[serde(rename = "WorldName", default, skip_serializing_if = "Option::is_none")]
    pub world_name: Option<String>,
    #[serde(rename = "WorldHex", default, skip_serializing_if = "Option::is_none")]
    pub world_hex: Option<String>,
    #[serde(rename = "WorldUwp", default, skip_serializing_if = "Option::is_none")]
    pub world_uwp: Option<String>,
    #[serde(
        rename = "WorldRemarks",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub world_remarks: Option<String>,
    #[serde(rename = "WorldIx", default, skip_serializing_if = "Option::is_none")]
    pub world_ix: Option<String>,
    #[serde(rename = "WorldEx", default, skip_serializing_if = "Option::is_none")]
    pub world_ex: Option<String>,
    #[serde(rename = "WorldCx", default, skip_serializing_if = "Option::is_none")]
    pub world_cx: Option<String>,
    #[serde(rename = "WorldPbg", default, skip_serializing_if = "Option::is_none")]
    pub world_pbg: Option<String>,
    #[serde(
        rename = "WorldAllegiance",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub world_allegiance: Option<String>,
    #[serde(
        rename = "WorldCredits",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub world_credits: Option<String>,

    #[serde(
        rename = "ProductPublisher",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub product_publisher: Option<String>,
    #[serde(
        rename = "ProductTitle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub product_title: Option<String>,
    #[serde(
        rename = "ProductAuthor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub product_author: Option<String>,
    #[serde(
        rename = "ProductRef",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub product_ref: Option<String>,
}

impl CreditsResult {
    /// `<Data>…</Data>` — the reference `CreditsResult` XML (`CreditsHandler.cs`).
    /// Every member is a child element in declaration order; null members are
    /// dropped (the .NET serializer omits nulls). `SectorX`/`SectorY` always
    /// emit (non-nullable ints).
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\"?>\n<Data>");
        xml_el_opt(&mut out, "Credits", &self.credits);
        xml_el(&mut out, "SectorX", &self.sector_x.to_string());
        xml_el(&mut out, "SectorY", &self.sector_y.to_string());
        xml_el_opt(&mut out, "SectorName", &self.sector_name);
        xml_el_opt(&mut out, "SectorAuthor", &self.sector_author);
        xml_el_opt(&mut out, "SectorSource", &self.sector_source);
        xml_el_opt(&mut out, "SectorPublisher", &self.sector_publisher);
        xml_el_opt(&mut out, "SectorCopyright", &self.sector_copyright);
        xml_el_opt(&mut out, "SectorRef", &self.sector_ref);
        xml_el_opt(&mut out, "SectorMilieu", &self.sector_milieu);
        xml_el_opt(&mut out, "SectorTags", &self.sector_tags);
        xml_el_opt(&mut out, "RouteCredits", &self.route_credits);
        xml_el_opt(&mut out, "SubsectorName", &self.subsector_name);
        xml_el_opt(&mut out, "SubsectorIndex", &self.subsector_index);
        xml_el_opt(&mut out, "SubsectorCredits", &self.subsector_credits);
        xml_el_opt(&mut out, "WorldName", &self.world_name);
        xml_el_opt(&mut out, "WorldHex", &self.world_hex);
        xml_el_opt(&mut out, "WorldUwp", &self.world_uwp);
        xml_el_opt(&mut out, "WorldRemarks", &self.world_remarks);
        xml_el_opt(&mut out, "WorldIx", &self.world_ix);
        xml_el_opt(&mut out, "WorldEx", &self.world_ex);
        xml_el_opt(&mut out, "WorldCx", &self.world_cx);
        xml_el_opt(&mut out, "WorldPbg", &self.world_pbg);
        xml_el_opt(&mut out, "WorldAllegiance", &self.world_allegiance);
        xml_el_opt(&mut out, "WorldCredits", &self.world_credits);
        xml_el_opt(&mut out, "ProductPublisher", &self.product_publisher);
        xml_el_opt(&mut out, "ProductTitle", &self.product_title);
        xml_el_opt(&mut out, "ProductAuthor", &self.product_author);
        xml_el_opt(&mut out, "ProductRef", &self.product_ref);
        out.push_str("</Data>");
        out
    }
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

/// The canonical travellermap.com `/api/search` envelope:
/// `{"Results":{"Count":N,"Items":[{"World":{…}},{"Sector":{…}},…]}}`. PascalCase
/// and structure match the reference `SearchHandler` exactly so third-party tools
/// that target the documented API parse our responses unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    #[serde(rename = "Results")]
    pub results: SearchResultsBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResultsBody {
    #[serde(rename = "Count")]
    pub count: usize,
    #[serde(rename = "Items")]
    pub items: Vec<SearchItem>,
}

/// One hit — exactly one of `World`/`Sector`/`Subsector`/`Label`. Externally
/// tagged, so it serializes as the public `{"World":{…}}` wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SearchItem {
    World(SearchWorld),
    Sector(SearchSector),
    Subsector(SearchSubsector),
    Label(SearchLabel),
}

/// A world hit. `HexX`/`HexY` are the *local* sector hex (1–32 / 1–40); combine
/// with `SectorX`/`SectorY` (the sector grid cell) for the absolute coordinate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchWorld {
    #[serde(rename = "HexX")]
    pub hex_x: i32,
    #[serde(rename = "HexY")]
    pub hex_y: i32,
    #[serde(rename = "Sector")]
    pub sector: String,
    #[serde(rename = "Uwp")]
    pub uwp: String,
    #[serde(rename = "SectorX")]
    pub sector_x: i32,
    #[serde(rename = "SectorY")]
    pub sector_y: i32,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SectorTags")]
    pub sector_tags: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchSector {
    #[serde(rename = "SectorX")]
    pub sector_x: i32,
    #[serde(rename = "SectorY")]
    pub sector_y: i32,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SectorTags")]
    pub sector_tags: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchSubsector {
    #[serde(rename = "Sector")]
    pub sector: String,
    /// Subsector letter `A`–`P`.
    #[serde(rename = "Index")]
    pub index: String,
    #[serde(rename = "SectorX")]
    pub sector_x: i32,
    #[serde(rename = "SectorY")]
    pub sector_y: i32,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SectorTags")]
    pub sector_tags: String,
}

/// A labeled-region hit (port of the reference `LabelResult`). `HexX`/`HexY` are
/// the *local* sector hex of the label's averaged centre coordinate; `Scale` is
/// the zoom bucket derived from the region radius (`>80→4`, `>40→8`, `>20→32`,
/// else `64`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchLabel {
    #[serde(rename = "HexX")]
    pub hex_x: i32,
    #[serde(rename = "HexY")]
    pub hex_y: i32,
    #[serde(rename = "Scale")]
    pub scale: i32,
    #[serde(rename = "SectorX")]
    pub sector_x: i32,
    #[serde(rename = "SectorY")]
    pub sector_y: i32,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SectorTags")]
    pub sector_tags: String,
}

impl SearchResults {
    /// `<results Count="N">` with one `<world>`/`<subsector>`/`<sector>`/`<label>`
    /// per item — the reference `SearchResults` XML (`SearchHandler.cs`). All
    /// fields are attributes: the `Item` base (`sectorX`, `sectorY`, `name`,
    /// `sectorTags`) first, then the subtype's (`hexX`/`hexY`/`sector`/`uwp` for
    /// worlds, `sector`/`index` for subsectors, `hexX`/`hexY`/`radius` for
    /// labels).
    pub fn to_xml(&self) -> String {
        let mut out = format!(
            "<?xml version=\"1.0\"?>\n<results Count=\"{}\">",
            self.results.count
        );
        for item in &self.results.items {
            match item {
                SearchItem::World(w) => {
                    out.push_str("<world");
                    xml_attr(&mut out, "sectorX", &w.sector_x.to_string());
                    xml_attr(&mut out, "sectorY", &w.sector_y.to_string());
                    xml_attr(&mut out, "name", &w.name);
                    xml_attr(&mut out, "sectorTags", &w.sector_tags);
                    xml_attr(&mut out, "hexX", &w.hex_x.to_string());
                    xml_attr(&mut out, "hexY", &w.hex_y.to_string());
                    xml_attr(&mut out, "sector", &w.sector);
                    xml_attr(&mut out, "uwp", &w.uwp);
                    out.push_str(" />");
                }
                SearchItem::Subsector(s) => {
                    out.push_str("<subsector");
                    xml_attr(&mut out, "sectorX", &s.sector_x.to_string());
                    xml_attr(&mut out, "sectorY", &s.sector_y.to_string());
                    xml_attr(&mut out, "name", &s.name);
                    xml_attr(&mut out, "sectorTags", &s.sector_tags);
                    xml_attr(&mut out, "sector", &s.sector);
                    xml_attr(&mut out, "index", &s.index);
                    out.push_str(" />");
                }
                SearchItem::Sector(s) => {
                    out.push_str("<sector");
                    xml_attr(&mut out, "sectorX", &s.sector_x.to_string());
                    xml_attr(&mut out, "sectorY", &s.sector_y.to_string());
                    xml_attr(&mut out, "name", &s.name);
                    xml_attr(&mut out, "sectorTags", &s.sector_tags);
                    out.push_str(" />");
                }
                SearchItem::Label(l) => {
                    out.push_str("<label");
                    xml_attr(&mut out, "sectorX", &l.sector_x.to_string());
                    xml_attr(&mut out, "sectorY", &l.sector_y.to_string());
                    xml_attr(&mut out, "name", &l.name);
                    xml_attr(&mut out, "sectorTags", &l.sector_tags);
                    xml_attr(&mut out, "hexX", &l.hex_x.to_string());
                    xml_attr(&mut out, "hexY", &l.hex_y.to_string());
                    xml_attr(&mut out, "radius", &l.scale.to_string());
                    out.push_str(" />");
                }
            }
        }
        out.push_str("</results>");
        out
    }
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
    /// Subsector display name the world's hex falls in (empty if unnamed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subsector: String,
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

impl RouteResult {
    /// Project to the documented public `/api/route` shape: a bare array of
    /// [`RouteStop`]s (port of `RouteHandler`'s `RouteStop` list). The private
    /// `coord`/aggregate fields are dropped; `SectorX/Y` + `HexX/Y` are
    /// recovered from each waypoint's absolute coordinate.
    pub fn to_public_stops(&self) -> Vec<RouteStop> {
        self.waypoints
            .iter()
            .map(|w| {
                // The pathfinding `coord` uses a naive `sector*32+col, sector*40+row`
                // packing (NOT the Astrometrics half-offset convention), so recover
                // the public sector/hex with the matching inverse — decoding it with
                // `coordinates_to_location` (Astrometrics) gives a wrong SectorY/HexX.
                // `hex` is the source of truth for the in-sector column/row.
                let (hx, hy) = crate::astrometrics::parse_hex(&w.hex).unwrap_or((0, 0));
                let sx = (w.coord.x - hx) / 32;
                let sy = (w.coord.y - hy) / 40;
                RouteStop {
                    sector: w.sector.clone(),
                    sector_x: sx,
                    sector_y: sy,
                    subsector: w.subsector.clone(),
                    name: w.name.clone(),
                    hex: w.hex.clone(),
                    hex_x: hx,
                    hex_y: hy,
                    uwp: w.uwp.clone(),
                    pbg: w.pbg.clone(),
                    zone: w.zone.clone(),
                    allegiance_name: w.allegiance.clone(),
                }
            })
            .collect()
    }
}

/// One stop in the public `/api/route` response (port of `RouteHandler`'s
/// `RouteStop`). PascalCase; the endpoint returns a bare array of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteStop {
    #[serde(rename = "Sector")]
    pub sector: String,
    #[serde(rename = "SectorX")]
    pub sector_x: i32,
    #[serde(rename = "SectorY")]
    pub sector_y: i32,
    /// Subsector display name the world's hex falls in (empty string if the
    /// subsector is unnamed) — mirrors the reference `RouteStop.SubsectorName`,
    /// which always emits the key.
    #[serde(rename = "Subsector")]
    pub subsector: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Hex")]
    pub hex: String,
    #[serde(rename = "HexX")]
    pub hex_x: i32,
    #[serde(rename = "HexY")]
    pub hex_y: i32,
    #[serde(rename = "UWP")]
    pub uwp: String,
    #[serde(rename = "PBG")]
    pub pbg: String,
    #[serde(rename = "Zone")]
    pub zone: String,
    #[serde(rename = "AllegianceName")]
    pub allegiance_name: String,
}

/// `<ArrayOfRouteStop><RouteStop>…</RouteStop>…</ArrayOfRouteStop>` — the
/// reference `/api/route` XML (`RouteHandler.cs` serializes a bare
/// `List<RouteStop>`, which the .NET `XmlSerializer` wraps as `ArrayOfRouteStop`).
/// Child elements in `RouteStop` declaration order: Sector, SectorX, SectorY,
/// Subsector, Name, Hex, HexX, HexY, UWP, PBG, Zone, AllegianceName.
pub fn route_stops_to_xml(stops: &[RouteStop]) -> String {
    let mut out = String::from("<?xml version=\"1.0\"?>\n<ArrayOfRouteStop>");
    for s in stops {
        out.push_str("<RouteStop>");
        xml_el(&mut out, "Sector", &s.sector);
        xml_el(&mut out, "SectorX", &s.sector_x.to_string());
        xml_el(&mut out, "SectorY", &s.sector_y.to_string());
        xml_el_str(&mut out, "Subsector", &s.subsector);
        xml_el(&mut out, "Name", &s.name);
        xml_el(&mut out, "Hex", &s.hex);
        xml_el(&mut out, "HexX", &s.hex_x.to_string());
        xml_el(&mut out, "HexY", &s.hex_y.to_string());
        xml_el_str(&mut out, "UWP", &s.uwp);
        xml_el_str(&mut out, "PBG", &s.pbg);
        xml_el_str(&mut out, "Zone", &s.zone);
        xml_el_str(&mut out, "AllegianceName", &s.allegiance_name);
        out.push_str("</RouteStop>");
    }
    out.push_str("</ArrayOfRouteStop>");
    out
}
