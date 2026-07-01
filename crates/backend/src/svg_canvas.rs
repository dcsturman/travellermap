//! A [`tmap_render::canvas::Canvas`] backend that emits SVG.
//!
//! The native counterpart to the frontend's `Canvas2d`: the same shared render
//! passes draw through this, producing an SVG document instead of pixels. Used by
//! the `/api/tile` endpoint to serve poster-style map tiles to external `<img>`
//! consumers (which render the SVG with the *viewer's* fonts — so this is a
//! local-deployment convenience, not pixel-faithful server-side rasterization).
//!
//! Coordinates the passes hand us are already in CSS pixels except for retained
//! [`Geometry`], which arrives in world space paired with an [`Affine`]; that
//! transform is emitted as `matrix(...)` on the geometry's `<path>`/group.

use std::cell::{Cell, RefCell};
use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

use tmap_render::canvas::{
    Affine, Canvas, Geometry, LineCap, LineJoin, PathCmd, Shadow, StrokeStyle, TextAlign,
};

/// Accumulates SVG markup for one rendered frame. Build it, pass `&self` to
/// [`tmap_render::render::draw_scene`], then call [`SvgCanvas::into_svg`].
pub struct SvgCanvas {
    body: RefCell<String>,
    /// Open `<g>` groups from `push_clip`, closed on `pop_clip`.
    clip_depth: Cell<u32>,
    /// Monotonic id source for `<clipPath>` defs.
    next_id: Cell<u32>,
}

impl SvgCanvas {
    pub fn new() -> Self {
        Self {
            body: RefCell::new(String::with_capacity(64 * 1024)),
            clip_depth: Cell::new(0),
            next_id: Cell::new(0),
        }
    }

    /// Wrap the accumulated body in an `<svg>` root sized `w`×`h` (CSS px). Any
    /// unbalanced clip groups are closed defensively.
    pub fn into_svg(self, w: f64, h: f64) -> String {
        let mut body = self.body.into_inner();
        for _ in 0..self.clip_depth.get() {
            body.push_str("</g>");
        }
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
             viewBox=\"0 0 {w} {h}\" preserveAspectRatio=\"xMidYMid meet\">{body}</svg>"
        )
    }

    fn out(&self) -> std::cell::RefMut<'_, String> {
        self.body.borrow_mut()
    }

    fn fresh_id(&self) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }
}

impl Default for SvgCanvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Compact number formatting (2 dp, trailing zeros trimmed) to keep the SVG small.
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let mut s = format!("{v:.2}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s.clear();
        s.push('0');
    }
    s
}

/// Escape text for an SVG text node / attribute value.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn anchor(align: TextAlign) -> &'static str {
    match align {
        TextAlign::Left => "start",
        TextAlign::Center => "middle",
        TextAlign::Right => "end",
    }
}

/// Append a `Geometry`'s commands as an SVG path `d` value. Arc commands assume
/// the current point is already at the arc's start (the render passes always
/// `move_to` there first), matching Canvas 2D `arc` semantics.
fn append_d(d: &mut String, cmds: &[PathCmd]) {
    let (mut cx0, mut cy0) = (0.0_f64, 0.0_f64);
    for cmd in cmds {
        match *cmd {
            PathCmd::MoveTo(x, y) => {
                let _ = write!(d, "M{} {}", num(x), num(y));
                cx0 = x;
                cy0 = y;
            }
            PathCmd::LineTo(x, y) => {
                let _ = write!(d, "L{} {}", num(x), num(y));
                cx0 = x;
                cy0 = y;
            }
            PathCmd::Bezier {
                c1x,
                c1y,
                c2x,
                c2y,
                x,
                y,
            } => {
                let _ = write!(
                    d,
                    "C{} {} {} {} {} {}",
                    num(c1x),
                    num(c1y),
                    num(c2x),
                    num(c2y),
                    num(x),
                    num(y)
                );
                cx0 = x;
                cy0 = y;
            }
            PathCmd::Close => d.push('Z'),
            PathCmd::Arc {
                cx,
                cy,
                r,
                start,
                end,
            } => {
                let delta = end - start;
                let sweep = u8::from(delta >= 0.0);
                let p1 = (cx + r * end.cos(), cy + r * end.sin());
                if delta.abs() >= TAU - 1e-9 {
                    // Full circle: SVG can't arc 360° in one segment — go via the
                    // antipode of the current (start) point, then back.
                    let (ax, ay) = (2.0 * cx - cx0, 2.0 * cy - cy0);
                    let _ = write!(
                        d,
                        "A{0} {0} 0 1 {1} {2} {3}",
                        num(r),
                        sweep,
                        num(ax),
                        num(ay)
                    );
                    let _ = write!(
                        d,
                        "A{0} {0} 0 1 {1} {2} {3}",
                        num(r),
                        sweep,
                        num(p1.0),
                        num(p1.1)
                    );
                } else {
                    let large = u8::from(delta.abs() > PI);
                    let _ = write!(
                        d,
                        "A{0} {0} 0 {1} {2} {3} {4}",
                        num(r),
                        large,
                        sweep,
                        num(p1.0),
                        num(p1.1)
                    );
                }
                cx0 = p1.0;
                cy0 = p1.1;
            }
        }
        d.push(' ');
    }
}

fn matrix(t: Affine) -> String {
    format!(
        "matrix({},{},{},{},{},{})",
        num(t.a),
        num(t.b),
        num(t.c),
        num(t.d),
        num(t.e),
        num(t.f)
    )
}

impl Canvas for SvgCanvas {
    fn clear(&self, color: &str, width: f64, height: f64) {
        let _ = write!(
            self.out(),
            "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
            num(width),
            num(height),
            esc(color)
        );
    }

    fn fill_circle(&self, x: f64, y: f64, radius: f64, color: &str) {
        let _ = write!(
            self.out(),
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
            num(x),
            num(y),
            num(radius),
            esc(color)
        );
    }

    fn fill_rect(&self, x: f64, y: f64, w: f64, h: f64, color: &str) {
        let _ = write!(
            self.out(),
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
            num(x),
            num(y),
            num(w),
            num(h),
            esc(color)
        );
    }

    fn stroke_arc(&self, cx: f64, cy: f64, r: f64, start: f64, end: f64, color: &str, width: f64) {
        let (sx, sy) = (cx + r * start.cos(), cy + r * start.sin());
        let mut d = format!("M{} {} ", num(sx), num(sy));
        append_d(
            &mut d,
            &[PathCmd::Arc {
                cx,
                cy,
                r,
                start,
                end,
            }],
        );
        let _ = write!(
            self.out(),
            "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"/>",
            d.trim(),
            esc(color),
            num(width)
        );
    }

    fn stroke_ellipse(
        &self,
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        rot: f64,
        color: &str,
        width: f64,
    ) {
        let deg = rot.to_degrees();
        let _ = write!(
            self.out(),
            "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"{}\" \
             stroke-width=\"{}\" transform=\"rotate({} {} {})\"/>",
            num(cx),
            num(cy),
            num(rx),
            num(ry),
            esc(color),
            num(width),
            num(deg),
            num(cx),
            num(cy)
        );
    }

    fn fill_polygons(&self, polys: &[Vec<(f64, f64)>], color: &str, alpha: f64) {
        let mut d = String::new();
        for poly in polys {
            let Some((first, rest)) = poly.split_first() else {
                continue;
            };
            let _ = write!(d, "M{} {}", num(first.0), num(first.1));
            for &(x, y) in rest {
                let _ = write!(d, "L{} {}", num(x), num(y));
            }
            d.push('Z');
        }
        if d.is_empty() {
            return;
        }
        let _ = write!(
            self.out(),
            "<path d=\"{}\" fill=\"{}\" fill-opacity=\"{}\"/>",
            d,
            esc(color),
            num(alpha)
        );
    }

    fn stroke_polyline(
        &self,
        points: &[(f64, f64)],
        color: &str,
        width: f64,
        close: bool,
        dash: &[f64],
    ) {
        let Some((first, rest)) = points.split_first() else {
            return;
        };
        let mut d = format!("M{} {}", num(first.0), num(first.1));
        for &(x, y) in rest {
            let _ = write!(d, "L{} {}", num(x), num(y));
        }
        if close {
            d.push('Z');
        }
        let mut out = self.out();
        let _ = write!(
            out,
            "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"",
            d,
            esc(color),
            num(width)
        );
        if !dash.is_empty() {
            let parts: Vec<String> = dash.iter().map(|d| num(*d)).collect();
            let _ = write!(out, " stroke-dasharray=\"{}\"", parts.join(","));
        }
        out.push_str("/>");
    }

    fn fill_text(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign) {
        let _ = write!(
            self.out(),
            "<text x=\"{}\" y=\"{}\" text-anchor=\"{}\" dominant-baseline=\"central\" \
             fill=\"{}\" style=\"font:{}\">{}</text>",
            num(x),
            num(y),
            anchor(align),
            esc(color),
            esc(font),
            esc(text)
        );
    }

    fn fill_text_top(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign) {
        let _ = write!(
            self.out(),
            "<text x=\"{}\" y=\"{}\" text-anchor=\"{}\" dominant-baseline=\"text-before-edge\" \
             fill=\"{}\" style=\"font:{}\">{}</text>",
            num(x),
            num(y),
            anchor(align),
            esc(color),
            esc(font),
            esc(text)
        );
    }

    fn fill_text_rotated(
        &self,
        text: &str,
        x: f64,
        y: f64,
        color: &str,
        font: &str,
        rot: f64,
        scale_x: f64,
        scale_y: f64,
        align: TextAlign,
        shadow: Option<&Shadow>,
    ) {
        let mut out = self.out();
        let _ = write!(
            out,
            "<g transform=\"translate({} {}) rotate({}) scale({} {})\">",
            num(x),
            num(y),
            num(rot.to_degrees()),
            num(scale_x),
            num(scale_y)
        );
        // Hard drop shadow: a duplicate offset text behind the main one (blur is
        // not reproduced — it's a Candy-only flourish, absent from poster).
        if let Some(sh) = shadow {
            let _ = write!(
                out,
                "<text x=\"{}\" y=\"{}\" text-anchor=\"{}\" dominant-baseline=\"central\" \
                 fill=\"{}\" style=\"font:{}\">{}</text>",
                num(sh.dx),
                num(sh.dy),
                anchor(align),
                esc(&sh.color),
                esc(font),
                esc(text)
            );
        }
        let _ = write!(
            out,
            "<text x=\"0\" y=\"0\" text-anchor=\"{}\" dominant-baseline=\"central\" \
             fill=\"{}\" style=\"font:{}\">{}</text></g>",
            anchor(align),
            esc(color),
            esc(font),
            esc(text)
        );
    }

    fn draw_image(&self, _url: &str, _dx: f64, _dy: f64, _dw: f64, _dh: f64, _alpha: f64) {
        // No-op: poster style uses no raster textures, and an SVG loaded via
        // <img> can't fetch external resources anyway. (A future raster backend,
        // or data-URI embedding, would handle Candy textures.)
    }

    fn fill_geometry(&self, g: &Geometry, transform: Affine, color: &str, alpha: f64) {
        let mut d = String::new();
        append_d(&mut d, g.cmds());
        if d.trim().is_empty() {
            return;
        }
        let mut out = self.out();
        let _ = write!(
            out,
            "<path d=\"{}\" transform=\"{}\" fill=\"{}\"",
            d.trim(),
            matrix(transform),
            esc(color)
        );
        if alpha != 1.0 {
            let _ = write!(out, " fill-opacity=\"{}\"", num(alpha));
        }
        out.push_str("/>");
    }

    fn stroke_geometry(
        &self,
        g: &Geometry,
        transform: Affine,
        color: &str,
        stroke: &StrokeStyle,
        clip: Option<&Geometry>,
    ) {
        let mut d = String::new();
        append_d(&mut d, g.cmds());
        if d.trim().is_empty() {
            return;
        }
        let cap = match stroke.cap {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
        };
        let join = match stroke.join {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
        };
        let mut out = self.out();
        // Wrap in a transformed group so an optional clip (under the SAME
        // transform) and the stroked path share one coordinate space.
        let _ = write!(out, "<g transform=\"{}\">", matrix(transform));
        let clip_attr = if let Some(cg) = clip {
            let mut cd = String::new();
            append_d(&mut cd, cg.cmds());
            let id = self.fresh_id();
            let _ = write!(
                out,
                "<clipPath id=\"tc{id}\" clipPathUnits=\"userSpaceOnUse\"><path d=\"{}\"/></clipPath>",
                cd.trim()
            );
            format!(" clip-path=\"url(#tc{id})\"")
        } else {
            String::new()
        };
        let _ = write!(
            out,
            "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" \
             stroke-linecap=\"{}\" stroke-linejoin=\"{}\"{}/></g>",
            d.trim(),
            esc(color),
            num(stroke.width),
            cap,
            join,
            clip_attr
        );
    }

    fn draw_border_group(
        &self,
        fills: &[&Geometry],
        stroke: &Geometry,
        transform: Affine,
        fill: Option<&str>,
        alpha: f64,
        stroke_color: &str,
        stroke_style: &StrokeStyle,
        clip: bool,
    ) {
        // Union of the per-sector fills = one path with many subpaths.
        let mut fd = String::new();
        for g in fills {
            append_d(&mut fd, g.cmds());
        }
        let fd = fd.trim().to_string();
        let mut sd = String::new();
        append_d(&mut sd, stroke.cmds());
        let sd = sd.trim().to_string();
        let cap = match stroke_style.cap {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
        };
        let join = match stroke_style.join {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
        };
        let mut out = self.out();
        let _ = write!(out, "<g transform=\"{}\">", matrix(transform));
        if let Some(color) = fill {
            if !fd.is_empty() {
                let _ = write!(out, "<path d=\"{fd}\" fill=\"{}\"", esc(color));
                if alpha != 1.0 {
                    let _ = write!(out, " fill-opacity=\"{}\"", num(alpha));
                }
                out.push_str("/>");
            }
        }
        if !sd.is_empty() {
            let clip_attr = if clip && !fd.is_empty() {
                let id = self.fresh_id();
                let _ = write!(
                    out,
                    "<clipPath id=\"tc{id}\" clipPathUnits=\"userSpaceOnUse\"><path d=\"{fd}\"/></clipPath>"
                );
                format!(" clip-path=\"url(#tc{id})\"")
            } else {
                String::new()
            };
            let _ = write!(
                out,
                "<path d=\"{sd}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" \
                 stroke-linecap=\"{}\" stroke-linejoin=\"{}\"{}/>",
                esc(stroke_color),
                num(stroke_style.width),
                cap,
                join,
                clip_attr
            );
        }
        out.push_str("</g>");
    }

    fn push_clip(&self, clip: &Geometry) {
        let mut cd = String::new();
        append_d(&mut cd, clip.cmds());
        let id = self.fresh_id();
        let mut out = self.out();
        let _ = write!(
            out,
            "<clipPath id=\"tc{id}\" clipPathUnits=\"userSpaceOnUse\"><path d=\"{}\"/></clipPath>\
             <g clip-path=\"url(#tc{id})\">",
            cd.trim()
        );
        self.clip_depth.set(self.clip_depth.get() + 1);
    }

    fn pop_clip(&self) {
        if self.clip_depth.get() > 0 {
            self.out().push_str("</g>");
            self.clip_depth.set(self.clip_depth.get() - 1);
        }
    }
}
