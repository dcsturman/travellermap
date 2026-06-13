//! Printable jump-route sheet — builds a standalone HTML document for a computed
//! route (opened in a new window and printed). Mirrors the reference
//! `print/route.html`: a titled header, a Jump-N summary, and one row per stop
//! with the world's location, starport class, gas-giant flag, travel zone, and
//! full allegiance name. Pure string generation — no `web-sys`.

use tmap_core::dto::RouteResult;

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

    let mut rows = String::new();
    for (i, w) in wps.iter().enumerate() {
        let last = i + 1 == wps.len();
        let leg = if last {
            String::new()
        } else {
            format!("<div class=leg>{}</div>", w.coord.hex_distance(wps[i + 1].coord))
        };
        let starport = w
            .uwp
            .chars()
            .next()
            .filter(|c| *c != '?')
            .map(|c| format!("Class {c}"))
            .unwrap_or_default();
        let gas_giant = w.pbg.as_bytes().get(2).is_some_and(|&b| b > b'0' && b != b'?');
        let (zone_txt, zone_cls) = match w.zone.as_str() {
            "A" => ("Amber Zone", "amber"),
            "R" => ("Red Zone", "red"),
            _ => ("", ""),
        };
        rows.push_str(&format!(
            "<div class=\"item{last_cls}\">\
               <div class=rail><div class=dot></div>{leg}</div>\
               <div class=content>\
                 <div class=name>{name}</div>\
                 <div class=detail>\
                   <span class=sh>{sector} {hex}</span>\
                   <span class=sp>{starport}</span>\
                   <span class=gg>{gg}</span>\
                   <span class=\"zone {zone_cls}\">{zone_txt}</span>\
                   <span class=al>{alleg}</span>\
                 </div>\
               </div>\
             </div>",
            last_cls = if last { " last" } else { "" },
            name = esc(&w.name),
            sector = esc(&w.sector),
            hex = w.hex,
            gg = if gas_giant { "Gas Giant" } else { "" },
            alleg = esc(&w.allegiance),
        ));
    }

    format!(
        "<!DOCTYPE html><html><head><meta charset=utf-8><title>{doc_title}</title><style>\
           body{{font:14px Arial,Helvetica,sans-serif;color:#000;margin:28px;max-width:8in;}}\
           h1{{font-size:26px;border-bottom:3px solid #000;padding-bottom:6px;margin:0;}}\
           h1 small{{font-size:60%;}}\
           h2{{font-size:18px;margin:16px 0 20px;}}\
           .item{{display:flex;align-items:stretch;}}\
           .rail{{width:46px;flex:none;position:relative;\
             background:linear-gradient(#2e7d2e,#2e7d2e) no-repeat;background-position:25px 0;background-size:4px 100%;}}\
           .item.last .rail{{background-image:none;}}\
           .rail .dot{{position:absolute;top:3px;left:19px;width:16px;height:16px;border-radius:50%;background:#2e7d2e;}}\
           .rail .leg{{position:absolute;left:2px;top:26px;font-weight:bold;}}\
           .content{{padding:0 0 16px 6px;}}\
           .name{{font-weight:bold;font-size:18px;line-height:1.1;}}\
           .detail{{display:flex;font-size:14px;margin-top:2px;}}\
           .detail .sh{{width:190px;}}.detail .sp{{width:95px;}}.detail .gg{{width:95px;}}\
           .detail .zone{{width:120px;font-weight:bold;}}.detail .al{{flex:1;}}\
           .zone.amber{{color:#d9a400;}}.zone.red{{color:#cc0000;}}\
           .footer{{margin-top:22px;font-size:11px;color:#333;font-style:italic;}}\
           @media print{{body{{margin:0;}}}}\
         </style></head><body>\
         <h1>{h1}</h1>\
         <h2>Jump-{jump}, {parsecs} parsecs, {jumps} jumps</h2>\
         {rows}\
         <div class=footer>Route planning is a benefit of membership in the Travellers' Aid Society. \
           The <i>Traveller</i> game is owned by Mongoose Publishing. Data: travellermap.com community.</div>\
         <script>window.onload=function(){{window.print();}}</script>\
         </body></html>",
        parsecs = route.parsecs,
        jumps = route.jumps,
    )
}
