//! Printable world data sheet — a standalone HTML document for one world (opened
//! in a new tab as a Blob URL and self-printed). Faithful port of the reference
//! `print/world.html` `#wds-world-template`: title with subsector/sector links,
//! Allegiance, System (stars + gas-giants/belts/other-worlds + bases), the UWP
//! decode table with its brace image, {Ix}/(Ex)/[Cx], Total Population, Nobility,
//! Remarks, and Travel Zone + TAS rating. Pure string generation — no `web-sys`.
//!
//! The jump-N neighborhood table from the reference sheet is intentionally
//! omitted here (that's the per-J jump-map feature).

use tmap_core::world_util::{decode_world, from_hex};

use crate::world_panel::SelectedWorld;

/// Decorative grouping brace over the UWP glyphs (reference base64 SVG, 144×49).
const UWP_BRACE: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNDQgNDkiIGhlaWdodD0iNDkiIHdpZHRoPSIxNDQiPjxwYXRoIGQ9Ik0wIDQ4di0xaDU2VjBoMnY0OUgwdi0xem03Mi0yMy41VjBoMnY0N2g3MHYySDcyVjI0LjV6TTAgMzR2LTFoNDBWMGgydjM1SDB2LTF6bTg4LTE2LjVWMGgydjMzaDU0djJIODhWMTcuNXpNMCAyMHYtMWgyNFYwaDJ2MjFIMHYtMXptMTA0LTkuNVYwaDJ2MTloMzh2MmgtNDBWMTAuNXpNMCA2VjVoOFYwaDJ2N0gwVjZ6bTEzNi0yLjVWMGgydjVoNnYyaC04VjMuNXoiLz48L3N2Zz4=";
/// Decorative grouping brace over the (Ex)/[Cx] glyphs (reference base64 SVG, 96×21).
const EXCX_BRACE: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCA5NiAyMSIgaGVpZ2h0PSIyMSIgd2lkdGg9Ijk2Ij48cGF0aCBkPSJNMCAyMHYtMWg0MFYwaDJ2MjFIMHYtMXptNTYtOS41VjBoMnYxOWgzOHYySDU2VjEwLjV6TTAgNlY1aDI0VjBoMnY3SDBWNnptNzItMi41VjBoMnY1aDIydjJINzJWMy41eiIvPjwvc3ZnPg==";

/// Minimal HTML text escaping for interpolated names/values.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One `.wds-decode` table wrapping the given inner `<tr>` rows.
fn decode_table(inner: &str) -> String {
    format!("<table class=wds-decode>{inner}</table>")
}

/// Render a full printable HTML document for the selected world. Returns an empty
/// string if there is no UWP to decode.
pub fn build_world_print_html(sel: &SelectedWorld) -> String {
    let w = &sel.world;
    if w.uwp.is_empty() {
        return String::new();
    }
    let d = decode_world(w);

    let name = if w.name.is_empty() {
        "(Unnamed)".to_string()
    } else {
        esc(&w.name)
    };
    let doc_title = format!(
        "{} - World Data Sheet",
        if w.name.is_empty() { "World" } else { &w.name }
    );
    let h1 = format!(
        "{name} <small>/ {sub} ({sector} {hex})</small>",
        sub = esc(&sel.subsector),
        sector = esc(&sel.sector_name),
        hex = esc(&w.hex),
    );

    let mut blocks = String::new();

    // Allegiance.
    if !w.allegiance.is_empty() {
        let full = d
            .allegiance_name
            .clone()
            .unwrap_or_else(|| w.allegiance.clone());
        blocks.push_str(&decode_table(&format!(
            "<tr><td>Allegiance:<td><big>{}</big> ({})",
            esc(&full),
            esc(&w.allegiance)
        )));
    }

    // System: stars, gas giants / belts / other worlds, bases.
    let stars: String = d
        .stars
        .iter()
        .map(|s| {
            format!(
                "<span class=wds-star><big>{}</big> <small>{}</small></span> ",
                esc(&s.code),
                esc(&s.blurb)
            )
        })
        .collect();
    let gg = d
        .pbg
        .gas_giants
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());
    let belts = d
        .pbg
        .belts
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());
    let other = d
        .other_worlds
        .filter(|n| *n > 0)
        .map(|n| format!(" &mdash; <big>{n}</big> <small>Other Worlds</small>"))
        .unwrap_or_default();
    let bases: String = d
        .bases
        .iter()
        .map(|b| format!("<span class=wds-base>{}</span> ", esc(b)))
        .collect();
    blocks.push_str(&decode_table(&format!(
        "<tr><td>System:<td>\
           <div class=wds-stars>{stars}</div>\
           <div><big>{gg}</big> <small>Gas Giants</small> &mdash; \
                <big>{belts}</big> <small>Planetoid Belts</small>{other}</div>\
           <div class=wds-bases>{bases}</div>"
    )));

    // UWP decode table (glyph row + brace + four blurb rows).
    let u = &d.uwp;
    blocks.push_str(&decode_table(&format!(
        "<tr><td>Universal World Profile:<td>{sp}<td>{sz}<td>{at}<td>{hy}<td>{po}<td>{go}<td>{la}<td>&ndash;<td>{te}<td>\
         <tr><td>Starport ({spb})<td colspan=9 rowspan=4><img width=144 height=49 src=\"{brace}\" alt=\"\"><td>Technology Level ({teb})\
         <tr><td>Size ({szb})<td>Law Level ({lab})\
         <tr><td>Atmosphere ({atb})<td>Government Type ({gob})\
         <tr><td>Hydrosphere ({hyb})<td>Population ({pob})",
        sp = esc(&u.starport.code), sz = esc(&u.size.code), at = esc(&u.atmosphere.code),
        hy = esc(&u.hydrographics.code), po = esc(&u.population.code), go = esc(&u.government.code),
        la = esc(&u.law.code), te = esc(&u.tech.code),
        spb = esc(&u.starport.blurb), teb = esc(&u.tech.blurb), szb = esc(&u.size.blurb),
        lab = esc(&u.law.blurb), atb = esc(&u.atmosphere.blurb), gob = esc(&u.government.blurb),
        hyb = esc(&u.hydrographics.blurb), pob = esc(&u.population.blurb),
        brace = UWP_BRACE,
    )));

    // Importance {Ix}.
    if let Some(ix) = &d.importance {
        blocks.push_str(&decode_table(&format!(
            "<tr><td>Importance {{Ix}}:<td>{{<td>{}<td>}} <td> <span class=wds-imp-blurb>{}</span>",
            esc(&ix.imp),
            esc(ix.blurb.as_deref().unwrap_or("")),
        )));
    }

    // Economics (Ex).
    if let Some(ex) = &d.economics {
        blocks.push_str(&decode_table(&format!(
            "<tr><td>Economics (Ex):<td>(<td>{res}<td>{lab}<td>{inf}<td>{eff}<td>)<td>\
             <tr><td>Resources ({resb})<td colspan=6 rowspan=2><img width=96 height=21 src=\"{brace}\" alt=\"\"><td>Efficiency ({effb})\
             <tr><td>Labor ({labb})<td>Infrastructure ({infb})",
            res = esc(&ex.resources.code), lab = esc(&ex.labor.code), inf = esc(&ex.infrastructure.code), eff = esc(&ex.efficiency.code),
            resb = esc(&ex.resources.blurb), effb = esc(&ex.efficiency.blurb), labb = esc(&ex.labor.blurb), infb = esc(&ex.infrastructure.blurb),
            brace = EXCX_BRACE,
        )));
    }

    // Culture [Cx].
    if let Some(cx) = &d.culture {
        blocks.push_str(&decode_table(&format!(
            "<tr><td>Culture [Cx]:<td>[<td>{het}<td>{acc}<td>{str}<td>{sym}<td>]<td>\
             <tr><td>Heterogeneity ({hetb})<td colspan=6 rowspan=2><img width=96 height=21 src=\"{brace}\" alt=\"\"><td>Symbols ({symb})\
             <tr><td>Acceptance ({accb})<td>Strangeness ({strb})",
            het = esc(&cx.heterogeneity.code), acc = esc(&cx.acceptance.code), str = esc(&cx.strangeness.code), sym = esc(&cx.symbols.code),
            hetb = esc(&cx.heterogeneity.blurb), symb = esc(&cx.symbols.blurb), accb = esc(&cx.acceptance.blurb), strb = esc(&cx.strangeness.blurb),
            brace = EXCX_BRACE,
        )));
    }

    // Total Population (PopMult × 10^PopExp = Total).
    if let Some(total) = &d.total_population {
        let pop_exp = from_hex(w.uwp.chars().nth(4).unwrap_or('?'));
        let pop_mult = if pop_exp > 0 && d.pbg.pop_mult == 0 {
            1
        } else {
            d.pbg.pop_mult
        };
        blocks.push_str(&decode_table(&format!(
            "<tr><td style=\"vertical-align:bottom\">Total Population:\
             <td><big>{pop_mult} &times; 10<sup>{pop_exp}</sup> = {total}</big>",
        )));
    }

    // Nobility.
    if !d.nobility.is_empty() {
        let nobles: String = d
            .nobility
            .iter()
            .map(|n| format!("<span class=wds-noble>{}</span> ", esc(&n.blurb)))
            .collect();
        blocks.push_str(&decode_table(&format!(
            "<tr><td>Nobility:<td><big class=wds-nobility>{nobles}</big>"
        )));
    }

    // Remarks.
    let remarks: String = d
        .remarks
        .iter()
        .filter(|r| !r.blurb.is_empty())
        .map(|r| {
            format!(
                "<span class=nowrap><big>{}</big> <small>{}</small></span>&#x20;",
                esc(&r.code),
                esc(&r.blurb)
            )
        })
        .collect();
    if !remarks.is_empty() {
        blocks.push_str(&decode_table(&format!(
            "<tr><td>Remarks:<td style=\"white-space:normal\">{remarks}"
        )));
    }

    // Travel Zone + TAS rating.
    let z = &d.zone;
    blocks.push_str(&decode_table(&format!(
        "<tr><td>Travel Zone:<td><big class=\"wds-zone-{cls}\">{rule}</big> <br> \
         <small>Travellers' Aid Society Rating: {rating}</small>",
        cls = z.class_name,
        rule = z.rule,
        rating = z.rating,
    )));

    format!(
        "<!DOCTYPE html><html><head><meta charset=utf-8><title>{doc_title}</title>\
         <style>\
           @import url('https://fonts.googleapis.com/css?family=PT+Sans+Narrow');\
           html,body{{margin:0;padding:0;}}\
           body{{padding:0.5in;background:#fff;color:#000;font:14px Helvetica,Arial,sans-serif;}}\
           h1{{font-size:26px;margin:0 0 6px;}}h1 small{{font-size:65%;font-weight:normal;color:#333;}}\
           td{{vertical-align:top;}}\
           .nowrap{{white-space:nowrap;display:inline-block;}}\
           .wds-decode{{white-space:nowrap;margin:10px 0;border-collapse:collapse;}}\
           .wds-decode tr:first-of-type td{{padding-bottom:4px;font-weight:bold;}}\
           .wds-decode td{{padding:0;width:16px;font-size:14px;line-height:14px;text-align:center;overflow:hidden;}}\
           .wds-decode img{{margin-top:2px;}}\
           .wds-decode td:first-of-type{{text-align:right;padding-right:4px;font-size:11px;width:160px;}}\
           .wds-decode td:last-of-type{{text-align:left;padding-left:4px;font-size:11px;width:auto;}}\
           .wds-imp-blurb{{font-weight:normal;}}\
           .wds-stars .wds-star:after,.wds-bases .wds-base:after,.wds-nobility .wds-noble:after{{content:' \\2014 ';}}\
           .wds-stars .wds-star:last-child:after,.wds-bases .wds-base:last-child:after,.wds-nobility .wds-noble:last-child:after{{content:'';}}\
           .wds-zone-amber{{background:#FFCC00;padding:2px 20px;display:inline-block;}}\
           .wds-zone-red{{background:#E32736;padding:2px 20px;display:inline-block;color:#fff;}}\
           #footer{{margin-top:24px;text-align:center;font-size:8pt;color:#222;}}\
           @media print{{@page{{size:portrait;margin:0.5in;}}body{{padding:0;-webkit-print-color-adjust:exact;print-color-adjust:exact;}}}}\
         </style></head><body>\
         <h1>{h1}</h1>\
         {blocks}\
         <div id=footer>The <i>Traveller</i> game in all forms is owned by Mongoose Publishing. \
           Copyright 1977 &ndash; 2024 Mongoose Publishing.</div>\
         <script>window.onload=function(){{window.print();}}</script>\
         </body></html>"
    )
}
