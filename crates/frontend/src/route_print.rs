//! Printable jump-route sheet — builds a standalone HTML document for a computed
//! route (opened in a new tab as a Blob URL and self-printed). Faithful port of
//! the reference `print/route.html`: Imperial Starburst + Marcellus title, a
//! Jump-N summary, one green-dotted row per stop (location, starport class,
//! gas-giant, travel zone, full allegiance name), and the TAS footer. Pure
//! string generation — no `web-sys`.

use tmap_core::dto::RouteResult;

/// The Imperial Starburst emblem, embedded at build time so the Blob-URL print
/// document needs no network/relative-path resolution. (Same asset the reference
/// loads from S3 / `res/app/ImperialStarburst.svg`.)
const STARBURST_SVG: &str = include_str!("../../../res/app/ImperialStarburst.svg");

/// Per-stop green dot + connecting line down (reference base64 SVG); the last
/// stop uses just the dot.
const DOT_LINE: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxMjgiIHdpZHRoPSIxNiIgaGVpZ2h0PSIxMjgiPjxjaXJjbGUgY3g9IjgiIGN5PSI4IiByPSI4IiBmaWxsPSIjMDQ4MTA0Ii8+PHBhdGggZmlsbD0iIzA0ODEwNCIgZD0iTTUgOGg2djEyOEg1eiIvPjwvc3ZnPg==";
const DOT_ONLY: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxNiIgd2lkdGg9IjE2IiBoZWlnaHQ9IjE2Ij48Y2lyY2xlIGN4PSI4IiBjeT0iOCIgcj0iOCIgZmlsbD0iIzA0ODEwNCIvPjwvc3ZnPg==";

/// Minimal HTML text escaping for interpolated world/sector/allegiance names.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Render a full printable HTML document for `route` at jump rating `jump`.
/// Returns an empty string if the route has no waypoints.
pub fn build_route_print_html(route: &RouteResult, jump: i32) -> String {
    let wps = &route.waypoints;
    let (Some(from), Some(to)) = (wps.first(), wps.last()) else {
        return String::new();
    };

    let doc_title = esc(&format!("{} to {}", from.name, to.name));
    let h1 = format!(
        "{} <small>({} {})</small> to {} <small>({} {})</small>",
        esc(&from.name), esc(&from.sector), from.hex,
        esc(&to.name), esc(&to.sector), to.hex,
    );
    // Inline the starburst markup (drop the XML/DOCTYPE prolog before <svg>).
    let star = STARBURST_SVG.find("<svg").map(|i| &STARBURST_SVG[i..]).unwrap_or("");

    let mut rows = String::new();
    for (i, w) in wps.iter().enumerate() {
        let last = i + 1 == wps.len();
        let leg = if last {
            String::new()
        } else {
            format!("<div class=item-distance>{}</div>", w.coord.hex_distance(wps[i + 1].coord))
        };
        let starport = w
            .uwp
            .chars()
            .next()
            .filter(|c| *c != '?')
            .map(|c| format!("Class {c}"))
            .unwrap_or_default();
        let gas_giant = w.pbg.as_bytes().get(2).is_some_and(|&b| b > b'0' && b != b'?');
        let zone_txt = match w.zone.as_str() {
            "A" => "Amber Zone",
            "R" => "Red Zone",
            _ => "",
        };
        rows.push_str(&format!(
            "<div class=\"item{last_cls}\">\
               {leg}\
               <div class=item-main>{name}</div>\
               <div class=item-location>\
                 <span class=item-sectorhex>{sector} {hex}</span>\
                 <span class=item-uwp>{starport}</span>\
                 <span class=item-pbg>{gg}</span>\
                 <span class=\"item-zone zone-{zone}\">{zone_txt}</span>\
                 <span class=item-alleg>{alleg}</span>\
               </div>\
             </div>",
            last_cls = if last { " last" } else { "" },
            name = esc(&w.name),
            sector = esc(&w.sector),
            hex = w.hex,
            gg = if gas_giant { "Gas Giant" } else { "" },
            zone = esc(&w.zone),
            alleg = esc(&w.allegiance),
        ));
    }

    format!(
        "<!DOCTYPE html><html><head><meta charset=utf-8><title>{doc_title}</title>\
         <style>\
           @import url('https://fonts.googleapis.com/css?family=Marcellus');\
           body{{padding:0.25in;font:12px Univers,Helvetica,Arial,sans-serif;color:#000;background:#fff;}}\
           h1{{display:flex;align-items:center;gap:8px;font-size:28px;line-height:35px;\
             padding-bottom:6px;border-bottom:4px solid #000;\
             font-family:Optima,Marcellus,'Times New Roman',serif;font-weight:bold;}}\
           h1 small{{font-size:18px;font-weight:bold;}}\
           h1 .star{{flex:none;width:40px;height:40px;}}\
           h1 .star svg{{width:40px;height:40px;display:block;}}\
           h2{{font-size:18px;margin:14px 0 18px 30px;}}\
           .item{{margin-left:30px;position:relative;padding:2px 0 5px 30px;margin-bottom:-5px;\
             background:url({dot_line}) no-repeat 3px 3px;}}\
           .item.last{{background-image:url({dot_only});}}\
           .item-distance{{position:absolute;left:0;top:22px;font-weight:bold;}}\
           .item-main{{font-size:16px;font-weight:bold;}}\
           .item-location{{margin-left:10pt;}}\
           .item-location span{{display:inline-block;}}\
           .item-sectorhex{{width:1.75in;}}.item-uwp{{width:0.6in;}}.item-pbg{{width:0.7in;}}\
           .item-zone{{width:0.9in;font-weight:bold;text-transform:uppercase;}}\
           .item-zone.zone-A{{color:#FFCC00;}}.item-zone.zone-R{{color:#E32736;}}\
           #footer{{margin:20px auto;width:5.5in;text-align:justify;font-size:12px;}}\
           @media print{{@page{{size:portrait;margin:0.25in;}}body{{padding:0;-webkit-print-color-adjust:exact;print-color-adjust:exact;}}}}\
         </style></head><body>\
         <h1><span class=star>{star}</span><span>{h1}</span></h1>\
         <h2>Jump-{jump}, {parsecs} parsecs, {jumps} jumps</h2>\
         <div id=routePath>{rows}</div>\
         <div id=footer>\
           Route planning is another Imperium-wide benefit of membership in the \
           <b>Travellers' Aid Society</b> &mdash; <i>Faithfully Serving Travellers Since The Year Zero</i>. \
           TAS facilities are available at your local class A or B starport. \
           The <i>Traveller</i> game in all forms is owned by Mongoose Publishing. \
           Copyright 1977 &ndash; 2024 Mongoose Publishing.\
         </div>\
         <script>window.onload=function(){{window.print();}}</script>\
         </body></html>",
        parsecs = route.parsecs,
        jumps = route.jumps,
        dot_line = DOT_LINE,
        dot_only = DOT_ONLY,
    )
}
