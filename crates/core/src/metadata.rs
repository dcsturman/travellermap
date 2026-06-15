//! Sector metadata: the documented `/api/metadata` shape (port of
//! `server/serialization/SectorMetadataSerializer.cs` + `Results.SectorMetadata`).
//!
//! This is the structured intermediate parsed **once** from a sector's `.xml`
//! (the reference's per-element parse re-reads the document for every field).
//! [`SectorMetadata`] serializes directly to the documented PascalCase JSON;
//! identity fields (X/Y/Tags/Abbreviation/Names/Selected/DataFile.Milieu) and the
//! worlds-derived `Allegiances` list are filled in by the caller, which has the
//! sector index entry and the world list.
//!
//! Field suppression mirrors the reference JSON serializer: null/`None` and
//! type-default values (`ShowLabel=true`, `WrapLabel=false`, offsets `0`) are
//! omitted; `Credits` always serializes as `[]` (an XML CDATA artifact).

use serde::Serialize;

use crate::astrometrics::parse_hex;
use crate::dto::SectorName;

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_zero_f(v: &f32) -> bool {
    *v == 0.0
}
fn is_zero_i(v: &i32) -> bool {
    *v == 0
}

/// The full `/api/metadata` document.
#[derive(Debug, Default, Serialize)]
pub struct SectorMetadata {
    #[serde(rename = "Selected", skip_serializing_if = "is_false")]
    pub selected: bool,
    #[serde(rename = "Tags")]
    pub tags: String,
    #[serde(rename = "Abbreviation", skip_serializing_if = "Option::is_none")]
    pub abbreviation: Option<String>,
    #[serde(rename = "Label", skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(rename = "Names")]
    pub names: Vec<SectorName>,
    /// Always `[]` in JSON (the reference serializes the credits CDATA node as an
    /// empty array). The credits *text* lives in [`Self::credits_text`].
    #[serde(rename = "Credits")]
    pub credits: Vec<u8>,
    #[serde(skip)]
    pub credits_text: Option<String>,
    #[serde(rename = "X")]
    pub x: i32,
    #[serde(rename = "Y")]
    pub y: i32,
    #[serde(rename = "Products")]
    pub products: Vec<Product>,
    #[serde(rename = "DataFile")]
    pub data_file: MetaDataFile,
    #[serde(rename = "Subsectors")]
    pub subsectors: Vec<MetaSubsector>,
    #[serde(rename = "Allegiances")]
    pub allegiances: Vec<MetaAllegiance>,
    #[serde(rename = "Stylesheet", skip_serializing_if = "Option::is_none")]
    pub stylesheet: Option<String>,
    #[serde(rename = "Labels")]
    pub labels: Vec<MetaLabel>,
    #[serde(rename = "Borders")]
    pub borders: Vec<MetaBorder>,
    #[serde(rename = "Regions")]
    pub regions: Vec<MetaBorder>,
    #[serde(rename = "Routes")]
    pub routes: Vec<MetaRoute>,
    /// Sector-local `<Allegiance>` overrides (Code → (Name, Base)); used by the
    /// caller to resolve the worlds-derived `Allegiances` list. Not serialized.
    #[serde(skip)]
    pub local_allegiances: Vec<MetaAllegiance>,
}

#[derive(Debug, Default, Serialize)]
pub struct MetaDataFile {
    #[serde(rename = "Title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "Author", skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(rename = "Source", skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(rename = "Publisher", skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(rename = "Copyright", skip_serializing_if = "Option::is_none")]
    pub copyright: Option<String>,
    #[serde(rename = "Milieu", skip_serializing_if = "Option::is_none")]
    pub milieu: Option<String>,
    #[serde(rename = "Ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Product {
    #[serde(rename = "Author", skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(rename = "Title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "Publisher", skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(rename = "Ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MetaSubsector {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Index")]
    pub index: String,
    #[serde(rename = "IndexNumber")]
    pub index_number: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetaAllegiance {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Code")]
    pub code: String,
    #[serde(rename = "Base", skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MetaBorder {
    #[serde(rename = "ShowLabel", skip_serializing_if = "is_true")]
    pub show_label: bool,
    #[serde(rename = "WrapLabel", skip_serializing_if = "is_false")]
    pub wrap_label: bool,
    #[serde(rename = "Color", skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(rename = "Allegiance", skip_serializing_if = "Option::is_none")]
    pub allegiance: Option<String>,
    #[serde(rename = "LabelPosition")]
    pub label_position: String,
    #[serde(rename = "LabelOffsetX", skip_serializing_if = "is_zero_f")]
    pub label_offset_x: f32,
    #[serde(rename = "LabelOffsetY", skip_serializing_if = "is_zero_f")]
    pub label_offset_y: f32,
    #[serde(rename = "Label", skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(rename = "Path")]
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct MetaRoute {
    #[serde(rename = "Start")]
    pub start: String,
    #[serde(rename = "End")]
    pub end: String,
    #[serde(rename = "StartOffsetX", skip_serializing_if = "is_zero_i")]
    pub start_offset_x: i32,
    #[serde(rename = "StartOffsetY", skip_serializing_if = "is_zero_i")]
    pub start_offset_y: i32,
    #[serde(rename = "EndOffsetX", skip_serializing_if = "is_zero_i")]
    pub end_offset_x: i32,
    #[serde(rename = "EndOffsetY", skip_serializing_if = "is_zero_i")]
    pub end_offset_y: i32,
    #[serde(rename = "Allegiance", skip_serializing_if = "Option::is_none")]
    pub allegiance: Option<String>,
    #[serde(rename = "Color", skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(rename = "Type", skip_serializing_if = "Option::is_none")]
    pub route_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MetaLabel {
    #[serde(rename = "Text")]
    pub text: String,
    #[serde(rename = "Hex")]
    pub hex: String,
    #[serde(rename = "Color", skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(rename = "Size", skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(rename = "Allegiance", skip_serializing_if = "Option::is_none")]
    pub allegiance: Option<String>,
}

fn attr(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attribute(name).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// `LabelPosition` for a border: the explicit attribute, else the bounding-box
/// centre of its path (`Border.PathString`'s setter computes `((min+max+1)/2)`).
fn label_position(explicit: Option<String>, hexes: &[String]) -> String {
    if let Some(p) = explicit {
        return p;
    }
    let coords: Vec<(i32, i32)> = hexes.iter().filter_map(|h| parse_hex(h)).collect();
    if coords.is_empty() {
        return "0000".to_string();
    }
    let (min_x, max_x) = coords.iter().fold((i32::MAX, i32::MIN), |(a, b), &(x, _)| (a.min(x), b.max(x)));
    let (min_y, max_y) = coords.iter().fold((i32::MAX, i32::MIN), |(a, b), &(_, y)| (a.min(y), b.max(y)));
    format!("{:02}{:02}", (min_x + max_x + 1) / 2, (min_y + max_y + 1) / 2)
}

fn parse_border(node: roxmltree::Node) -> MetaBorder {
    let hexes: Vec<String> = node
        .text()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    MetaBorder {
        show_label: node.attribute("ShowLabel").map(|v| v != "False" && v != "false").unwrap_or(true),
        wrap_label: matches!(node.attribute("WrapLabel"), Some("True") | Some("true")),
        color: attr(node, "Color"),
        allegiance: attr(node, "Allegiance"),
        label_position: label_position(attr(node, "LabelPosition"), &hexes),
        label_offset_x: attr(node, "LabelOffsetX").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        label_offset_y: attr(node, "LabelOffsetY").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        label: attr(node, "Label"),
        path: hexes.join(" "),
    }
}

/// Parse a sector's metadata `.xml` into [`SectorMetadata`] content (everything
/// except the caller-supplied identity + worlds-derived `Allegiances`).
pub fn parse_sector_metadata(xml: &str) -> SectorMetadata {
    let mut meta = SectorMetadata::default();
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return meta;
    };

    // Root <Sector> identity attributes (the rest comes from child elements).
    let root = doc.root_element();
    if root.has_tag_name("Sector") {
        meta.selected = matches!(root.attribute("Selected"), Some("True") | Some("true"));
        meta.tags = root.attribute("Tags").unwrap_or("").trim().to_owned();
        meta.abbreviation = attr(root, "Abbreviation");
        meta.label = attr(root, "Label");
    }

    for n in doc.descendants() {
        match n.tag_name().name() {
            "Subsector" => {
                if let (Some(index), Some(name)) = (attr(n, "Index"), n.text().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())) {
                    let index_number = index.chars().next().map(|c| (c.to_ascii_uppercase() as i32) - ('A' as i32)).unwrap_or(0);
                    meta.subsectors.push(MetaSubsector { name, index, index_number });
                }
            }
            "Product" => meta.products.push(Product {
                author: attr(n, "Author"),
                title: attr(n, "Title"),
                publisher: attr(n, "Publisher"),
                reference: attr(n, "Ref"),
            }),
            "Stylesheet" => {
                meta.stylesheet = n.text().map(str::to_owned).filter(|s| !s.trim().is_empty());
            }
            "Credits" => {
                let text: String = n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    meta.credits_text = Some(trimmed.to_owned());
                }
            }
            "Border" => meta.borders.push(parse_border(n)),
            "Region" => meta.regions.push(parse_border(n)),
            "Route" => meta.routes.push(MetaRoute {
                start: attr(n, "Start").unwrap_or_default(),
                end: attr(n, "End").unwrap_or_default(),
                start_offset_x: attr(n, "StartOffsetX").and_then(|s| s.parse().ok()).unwrap_or(0),
                start_offset_y: attr(n, "StartOffsetY").and_then(|s| s.parse().ok()).unwrap_or(0),
                end_offset_x: attr(n, "EndOffsetX").and_then(|s| s.parse().ok()).unwrap_or(0),
                end_offset_y: attr(n, "EndOffsetY").and_then(|s| s.parse().ok()).unwrap_or(0),
                allegiance: attr(n, "Allegiance"),
                color: attr(n, "Color"),
                route_type: attr(n, "Type"),
            }),
            "Label" => meta.labels.push(MetaLabel {
                text: n.text().map(|s| s.trim().to_owned()).unwrap_or_default(),
                hex: attr(n, "Hex").unwrap_or_default(),
                color: attr(n, "Color"),
                size: attr(n, "Size"),
                allegiance: attr(n, "Allegiance"),
            }),
            "Allegiance" => {
                if let Some(code) = attr(n, "Code") {
                    let name = n.text().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_default();
                    meta.local_allegiances.push(MetaAllegiance { name, code, base: attr(n, "Base") });
                }
            }
            _ => {}
        }
    }
    meta
}
