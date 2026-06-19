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

impl SectorMetadata {
    /// Serialize to the reference `/api/metadata` XML shape (`[XmlRoot("Sector")]`
    /// in `SectorMetaDataHandler.cs`) — the same data we emit as JSON, as XML.
    /// Member order matches the reference result class; empty `Labels`/`Borders`/
    /// `Regions`/`Routes` collections are omitted (the reference `ShouldSerialize*`).
    pub fn to_xml(&self) -> String {
        use crate::dto::{xml_attr, xml_el, xml_escape};
        let mut o = String::from("<?xml version=\"1.0\"?>\n<Sector");
        if self.selected {
            xml_attr(&mut o, "Selected", "true");
        }
        xml_attr(&mut o, "Tags", &self.tags);
        if let Some(a) = &self.abbreviation {
            xml_attr(&mut o, "Abbreviation", a);
        }
        if let Some(l) = &self.label {
            xml_attr(&mut o, "Label", l);
        }
        o.push('>');

        for n in &self.names {
            o.push_str("<Name");
            if let Some(lang) = &n.lang {
                xml_attr(&mut o, "Lang", lang);
            }
            if let Some(src) = &n.source {
                xml_attr(&mut o, "Source", src);
            }
            o.push('>');
            o.push_str(&xml_escape(&n.text));
            o.push_str("</Name>");
        }

        // Credits is a CDATA node (raw HTML); only present when non-empty.
        if let Some(c) = self.credits_text.as_deref().filter(|s| !s.is_empty()) {
            o.push_str("<Credits><![CDATA[");
            o.push_str(c);
            o.push_str("]]></Credits>");
        }

        xml_el(&mut o, "X", &self.x.to_string());
        xml_el(&mut o, "Y", &self.y.to_string());

        for p in &self.products {
            o.push_str("<Product");
            if let Some(v) = &p.author {
                xml_attr(&mut o, "Author", v);
            }
            if let Some(v) = &p.title {
                xml_attr(&mut o, "Title", v);
            }
            if let Some(v) = &p.publisher {
                xml_attr(&mut o, "Publisher", v);
            }
            if let Some(v) = &p.reference {
                xml_attr(&mut o, "Ref", v);
            }
            o.push_str(" />");
        }

        o.push_str("<DataFile");
        let df = &self.data_file;
        for (name, val) in [
            ("Title", &df.title),
            ("Author", &df.author),
            ("Source", &df.source),
            ("Publisher", &df.publisher),
            ("Copyright", &df.copyright),
            ("Milieu", &df.milieu),
            ("Ref", &df.reference),
        ] {
            if let Some(v) = val {
                xml_attr(&mut o, name, v);
            }
        }
        o.push_str(" />");

        // Subsectors and Allegiances are always serialized (no ShouldSerialize).
        o.push_str("<Subsectors>");
        for s in &self.subsectors {
            o.push_str("<Subsector");
            xml_attr(&mut o, "Index", &s.index);
            if s.name.is_empty() {
                o.push_str(" />");
            } else {
                o.push('>');
                o.push_str(&xml_escape(&s.name));
                o.push_str("</Subsector>");
            }
        }
        o.push_str("</Subsectors>");

        o.push_str("<Allegiances>");
        for a in &self.allegiances {
            o.push_str("<Allegiance");
            xml_attr(&mut o, "Code", &a.code);
            if let Some(b) = &a.base {
                xml_attr(&mut o, "Base", b);
            }
            o.push('>');
            o.push_str(&xml_escape(&a.name));
            o.push_str("</Allegiance>");
        }
        o.push_str("</Allegiances>");

        if let Some(ss) = &self.stylesheet {
            xml_el(&mut o, "Stylesheet", ss);
        }

        if !self.labels.is_empty() {
            o.push_str("<Labels>");
            for l in &self.labels {
                xml_label(&mut o, l);
            }
            o.push_str("</Labels>");
        }
        if !self.borders.is_empty() {
            o.push_str("<Borders>");
            for b in &self.borders {
                xml_border(&mut o, "Border", b);
            }
            o.push_str("</Borders>");
        }
        if !self.regions.is_empty() {
            o.push_str("<Regions>");
            for b in &self.regions {
                xml_border(&mut o, "Region", b);
            }
            o.push_str("</Regions>");
        }
        if !self.routes.is_empty() {
            o.push_str("<Routes>");
            for r in &self.routes {
                xml_route(&mut o, r);
            }
            o.push_str("</Routes>");
        }

        o.push_str("</Sector>");
        o
    }
}

fn xml_border(o: &mut String, tag: &str, b: &MetaBorder) {
    use crate::dto::{xml_attr, xml_escape};
    o.push('<');
    o.push_str(tag);
    if !b.show_label {
        xml_attr(o, "ShowLabel", "false");
    }
    if b.wrap_label {
        xml_attr(o, "WrapLabel", "true");
    }
    if let Some(c) = &b.color {
        xml_attr(o, "Color", c);
    }
    if let Some(a) = &b.allegiance {
        xml_attr(o, "Allegiance", a);
    }
    xml_attr(
        o,
        "LabelPosition",
        &label_position(b.label_position.clone(), &b.hexes),
    );
    if b.label_offset_x != 0.0 {
        xml_attr(o, "LabelOffsetX", &b.label_offset_x.to_string());
    }
    if b.label_offset_y != 0.0 {
        xml_attr(o, "LabelOffsetY", &b.label_offset_y.to_string());
    }
    if let Some(l) = &b.label {
        xml_attr(o, "Label", l);
    }
    o.push('>');
    o.push_str(&xml_escape(&b.hexes.join(" ")));
    o.push_str("</");
    o.push_str(tag);
    o.push('>');
}

fn xml_route(o: &mut String, r: &MetaRoute) {
    use crate::dto::xml_attr;
    o.push_str("<Route");
    xml_attr(o, "Start", &r.start);
    xml_attr(o, "End", &r.end);
    for (name, v) in [
        ("StartOffsetX", r.start_offset_x),
        ("StartOffsetY", r.start_offset_y),
        ("EndOffsetX", r.end_offset_x),
        ("EndOffsetY", r.end_offset_y),
    ] {
        if v != 0 {
            xml_attr(o, name, &v.to_string());
        }
    }
    if let Some(a) = &r.allegiance {
        xml_attr(o, "Allegiance", a);
    }
    if let Some(c) = &r.color {
        xml_attr(o, "Color", c);
    }
    if let Some(t) = &r.route_type {
        xml_attr(o, "Type", t);
    }
    o.push_str(" />");
}

fn xml_label(o: &mut String, l: &MetaLabel) {
    use crate::dto::{xml_attr, xml_escape};
    o.push_str("<Label");
    xml_attr(o, "Hex", &l.hex);
    if let Some(a) = &l.allegiance {
        xml_attr(o, "Allegiance", a);
    }
    if let Some(c) = &l.color {
        xml_attr(o, "Color", c);
    }
    if let Some(s) = &l.size {
        xml_attr(o, "Size", s);
    }
    if l.wrap {
        xml_attr(o, "Wrap", "true");
    }
    if l.offset_x != 0.0 {
        xml_attr(o, "OffsetX", &l.offset_x.to_string());
    }
    if l.offset_y != 0.0 {
        xml_attr(o, "OffsetY", &l.offset_y.to_string());
    }
    o.push('>');
    o.push_str(&xml_escape(&l.text));
    o.push_str("</Label>");
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

/// A border/region as parsed (**raw**): `label_position` is the explicit
/// `LabelPosition` attribute (`None` if absent), `hexes` the path. Both
/// projections read these — the render path wants the raw attribute, the public
/// JSON computes the bounding-box-centre fallback and joins the path. Serialized
/// manually so the public shape carries the *computed* `LabelPosition` + `Path`.
#[derive(Debug)]
pub struct MetaBorder {
    pub show_label: bool,
    pub wrap_label: bool,
    pub color: Option<String>,
    pub allegiance: Option<String>,
    pub label_position: Option<String>,
    pub label_offset_x: f32,
    pub label_offset_y: f32,
    pub label: Option<String>,
    pub hexes: Vec<String>,
    /// Document-order index across `<Border>` *and* `<Region>` combined. The
    /// render layer draws both in source order (interleaved); this lets the
    /// caller restore that order after they're split into `borders`/`regions`.
    pub seq: usize,
}

impl Serialize for MetaBorder {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        if !self.show_label {
            m.serialize_entry("ShowLabel", &false)?;
        }
        if self.wrap_label {
            m.serialize_entry("WrapLabel", &true)?;
        }
        if let Some(c) = &self.color {
            m.serialize_entry("Color", c)?;
        }
        if let Some(a) = &self.allegiance {
            m.serialize_entry("Allegiance", a)?;
        }
        m.serialize_entry(
            "LabelPosition",
            &label_position(self.label_position.clone(), &self.hexes),
        )?;
        if self.label_offset_x != 0.0 {
            m.serialize_entry("LabelOffsetX", &self.label_offset_x)?;
        }
        if self.label_offset_y != 0.0 {
            m.serialize_entry("LabelOffsetY", &self.label_offset_y)?;
        }
        if let Some(l) = &self.label {
            m.serialize_entry("Label", l)?;
        }
        m.serialize_entry("Path", &self.hexes.join(" "))?;
        m.end()
    }
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
    #[serde(rename = "Hex")]
    pub hex: String,
    #[serde(rename = "Allegiance", skip_serializing_if = "Option::is_none")]
    pub allegiance: Option<String>,
    #[serde(rename = "Color", skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(rename = "Size", skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(rename = "Wrap")]
    pub wrap: bool,
    #[serde(rename = "OffsetX", skip_serializing_if = "is_zero_f")]
    pub offset_x: f32,
    #[serde(rename = "OffsetY", skip_serializing_if = "is_zero_f")]
    pub offset_y: f32,
    #[serde(rename = "Text")]
    pub text: String,
}

fn attr(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attribute(name)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
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
    let (min_x, max_x) = coords
        .iter()
        .fold((i32::MAX, i32::MIN), |(a, b), &(x, _)| (a.min(x), b.max(x)));
    let (min_y, max_y) = coords
        .iter()
        .fold((i32::MAX, i32::MIN), |(a, b), &(_, y)| (a.min(y), b.max(y)));
    format!(
        "{:02}{:02}",
        (min_x + max_x + 1) / 2,
        (min_y + max_y + 1) / 2
    )
}

fn parse_border(node: roxmltree::Node, seq: usize) -> MetaBorder {
    // Keep only 4-digit numeric hex tokens (matches the reference + the render
    // border parse), so stray whitespace/garbage never enters the path.
    let hexes: Vec<String> = node
        .text()
        .unwrap_or("")
        .split_whitespace()
        .filter(|t| t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned)
        .collect();
    MetaBorder {
        show_label: node
            .attribute("ShowLabel")
            .map(|v| v != "False" && v != "false")
            .unwrap_or(true),
        wrap_label: matches!(node.attribute("WrapLabel"), Some("True") | Some("true")),
        color: attr(node, "Color"),
        allegiance: attr(node, "Allegiance"),
        label_position: attr(node, "LabelPosition"),
        label_offset_x: attr(node, "LabelOffsetX")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        label_offset_y: attr(node, "LabelOffsetY")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        label: attr(node, "Label"),
        hexes,
        seq,
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

    let mut border_seq = 0usize;
    for n in doc.descendants() {
        match n.tag_name().name() {
            // Direct `<Name>` children of `<Sector>`. The GET `/api/metadata` path
            // overwrites these from the (authoritative) sector index entry; they
            // matter for the POST round-trip, where the posted document is the only
            // source. Restricted to children of the root so it can't pick up an
            // unrelated nested `Name` element.
            "Name" if n.parent().is_some_and(|p| p.has_tag_name("Sector")) => {
                if let Some(text) = n
                    .text()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                {
                    meta.names.push(SectorName {
                        text,
                        lang: attr(n, "Lang"),
                        source: attr(n, "Source"),
                    });
                }
            }
            "Subsector" => {
                if let (Some(index), Some(name)) = (
                    attr(n, "Index"),
                    n.text()
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty()),
                ) {
                    let index_number = index
                        .chars()
                        .next()
                        .map(|c| (c.to_ascii_uppercase() as i32) - ('A' as i32))
                        .unwrap_or(0);
                    meta.subsectors.push(MetaSubsector {
                        name,
                        index,
                        index_number,
                    });
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
                let text: String = n
                    .descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    meta.credits_text = Some(trimmed.to_owned());
                }
            }
            "Border" => {
                meta.borders.push(parse_border(n, border_seq));
                border_seq += 1;
            }
            "Region" => {
                meta.regions.push(parse_border(n, border_seq));
                border_seq += 1;
            }
            "Route" => meta.routes.push(MetaRoute {
                start: attr(n, "Start").unwrap_or_default(),
                end: attr(n, "End").unwrap_or_default(),
                start_offset_x: attr(n, "StartOffsetX")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                start_offset_y: attr(n, "StartOffsetY")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                end_offset_x: attr(n, "EndOffsetX")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                end_offset_y: attr(n, "EndOffsetY")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                allegiance: attr(n, "Allegiance"),
                color: attr(n, "Color"),
                route_type: attr(n, "Type"),
            }),
            "Label" => meta.labels.push(MetaLabel {
                hex: attr(n, "Hex").unwrap_or_default(),
                allegiance: attr(n, "Allegiance"),
                color: attr(n, "Color"),
                size: attr(n, "Size"),
                wrap: matches!(n.attribute("Wrap"), Some("True") | Some("true")),
                offset_x: attr(n, "OffsetX")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0),
                offset_y: attr(n, "OffsetY")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0),
                text: n.text().map(|s| s.trim().to_owned()).unwrap_or_default(),
            }),
            "Allegiance" => {
                if let Some(code) = attr(n, "Code") {
                    let name = n
                        .text()
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_default();
                    meta.local_allegiances.push(MetaAllegiance {
                        name,
                        code,
                        base: attr(n, "Base"),
                    });
                }
            }
            _ => {}
        }
    }
    meta
}
