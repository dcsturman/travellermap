//! The `Canvas` graphics trait and its Canvas 2D (`web-sys`) backend.
//!
//! This is the swappable seam (mirrors the reference `AbstractGraphics`): all
//! map-drawing logic in `render/` calls `trait Canvas`, never `web-sys`
//! directly. Today the only impl is [`Canvas2d`] (browser-native rasterization
//! via `CanvasRenderingContext2d`); a future `WgpuCanvas` can drop in without
//! touching scene logic. See PORT_PLAN.md "Phase 15 — wgpu" decision.
//!
//! The seam has three layers of primitive:
//! - **Immediate shapes/text** — `fill_circle`/`fill_rect`/`fill_text`/… draw
//!   once in screen pixels.
//! - **Retained [`Geometry`]** — a path built once (in any coordinate space) via
//!   [`PathBuilder`] and re-`fill_geometry`/`stroke_geometry`d each frame under
//!   an [`Affine`] transform. This is the hot, cached map layer (borders, the hex
//!   grid, world dots): a pan re-transforms instead of re-emitting the path. On
//!   the Canvas2d backend a `Geometry` wraps a `Path2d`; a GPU backend wraps a
//!   tessellated vertex buffer — the render passes never see either.
//! - **Clip** — `push_clip`/`pop_clip` (screen-space) and the `clip` argument of
//!   `stroke_geometry` (world-space, paired with the stroked geometry).

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, Path2d};

#[derive(Clone, Copy)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl TextAlign {
    fn as_str(self) -> &'static str {
        match self {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        }
    }
}

/// 2×3 affine transform (`a b c d e f`, column-major like the Canvas 2D
/// `setTransform`): `x' = a·x + c·y + e`, `y' = b·x + d·y + f`. Used as the
/// **absolute** device transform a [`Geometry`] is drawn under, so it bakes in
/// the device-pixel-ratio (it replaces, not multiplies, the frame transform).
#[derive(Clone, Copy)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }
    /// Uniform scale + translate (the only shape the map's world→device
    /// transform ever takes): `a = d = scale`, `e = tx`, `f = ty`.
    pub fn scale_translate(scale: f64, tx: f64, ty: f64) -> Self {
        Self::new(scale, 0.0, 0.0, scale, tx, ty)
    }
}

/// Line cap/join for a [`StrokeStyle`] — only the two values the map uses.
#[derive(Clone, Copy, PartialEq)]
pub enum LineCap {
    Butt,
    Round,
}
#[derive(Clone, Copy, PartialEq)]
pub enum LineJoin {
    Miter,
    Round,
}

/// Stroke parameters for [`Canvas::stroke_geometry`]. `width` is in the
/// geometry's own coordinate units (the transform scales it).
#[derive(Clone, Copy)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
}

impl StrokeStyle {
    /// Plain stroke (butt cap, miter join) of the given width.
    pub fn plain(width: f64) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }
    }
    /// Rounded cap+join (the border outlines).
    pub fn round(width: f64) -> Self {
        Self {
            width,
            cap: LineCap::Round,
            join: LineJoin::Round,
        }
    }
}

/// A hard drop shadow for a single text draw (the Candy "eye-candy" name look:
/// `textBackgroundStyle = Shadow`).
pub struct Shadow {
    pub color: String,
    pub dx: f64,
    pub dy: f64,
    pub blur: f64,
}

/// Retained path geometry: built once via [`PathBuilder`], drawn many times. The
/// render passes treat it as opaque — on the Canvas2d backend it wraps a
/// `Path2d` (so a cached `Geometry` is just a cached browser path); a GPU backend
/// would wrap a tessellated mesh instead.
pub struct Geometry {
    path: Path2d,
}

impl Geometry {
    /// Backend accessor — only the [`Canvas2d`] impl in this module may reach the
    /// underlying `Path2d`; render passes never do.
    fn path(&self) -> &Path2d {
        &self.path
    }
}

/// Accumulates a [`Geometry`] from path commands. Mirrors the subset of the
/// `Path2d` API the map needs, in backend-neutral terms.
pub struct PathBuilder {
    path: Path2d,
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBuilder {
    pub fn new() -> Self {
        Self {
            // Path2d::new only fails on OOM, which we treat as fatal elsewhere too.
            path: Path2d::new().unwrap(),
        }
    }
    pub fn move_to(&self, x: f64, y: f64) {
        self.path.move_to(x, y);
    }
    pub fn line_to(&self, x: f64, y: f64) {
        self.path.line_to(x, y);
    }
    pub fn close(&self) {
        self.path.close_path();
    }
    /// Arc (full circle: `move_to(cx+r, cy)` first, then `arc(cx,cy,r,0,TAU)`).
    pub fn arc(&self, cx: f64, cy: f64, r: f64, start: f64, end: f64) {
        // arc only errors on a non-finite radius, which callers never pass.
        let _ = self.path.arc(cx, cy, r, start, end);
    }
    pub fn bezier_to(&self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        self.path.bezier_curve_to(c1x, c1y, c2x, c2y, x, y);
    }
    /// Append another (already-built) geometry's path — the per-frame combine of
    /// cached per-sector geometries into per-group geometries.
    pub fn add(&self, g: &Geometry) {
        self.path.add_path(g.path());
    }
    pub fn finish(self) -> Geometry {
        Geometry { path: self.path }
    }
}

/// Immediate-mode drawing surface. Coordinates are screen pixels (unless drawn
/// through a [`Geometry`] + [`Affine`]); colors are any CSS color string.
pub trait Canvas {
    fn clear(&self, color: &str, width: f64, height: f64);
    fn fill_circle(&self, x: f64, y: f64, radius: f64, color: &str);
    /// Filled axis-aligned rect (the Candy/Mongoose filled-UWP background box).
    fn fill_rect(&self, x: f64, y: f64, w: f64, h: f64, color: &str);
    /// Stroked open arc (the Candy travel-zone arcs).
    #[allow(clippy::too_many_arguments)]
    fn stroke_arc(&self, cx: f64, cy: f64, r: f64, start: f64, end: f64, color: &str, width: f64);
    /// Stroked ellipse rotated by `rot` radians (the gas-giant Saturn ring).
    #[allow(clippy::too_many_arguments)]
    fn stroke_ellipse(&self, cx: f64, cy: f64, rx: f64, ry: f64, rot: f64, color: &str, width: f64);
    /// Fill many closed polygons as one path at the given `alpha` (union,
    /// single pass — no seams between adjacent border hexes). `color` may be
    /// any CSS color (name or hex); `alpha` is applied via globalAlpha so no
    /// name→rgb table is needed.
    fn fill_polygons(&self, polys: &[Vec<(f64, f64)>], color: &str, alpha: f64);
    /// Stroke a polyline through `points`; `close` joins the last point to the
    /// first; a non-empty `dash` makes it dashed (canvas dash pattern).
    fn stroke_polyline(
        &self,
        points: &[(f64, f64)],
        color: &str,
        width: f64,
        close: bool,
        dash: &[f64],
    );
    /// Centered (middle-baseline) text at `(x, y)`.
    fn fill_text(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign);
    /// Top-baseline text (the hex coordinate just inside the top hex edge —
    /// reference TopCenter).
    fn fill_text_top(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign);
    /// Text rotated by `rot` radians about `(x, y)`, with independent
    /// horizontal/vertical scales (for the squished sector/subsector watermark
    /// labels — diagonal in most styles, horizontal + non-uniform in Candy), an
    /// `align` (most callers center; the Candy world name left-aligns at its
    /// origin) and an optional hard drop `shadow`.
    #[allow(clippy::too_many_arguments)]
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
    );
    /// Draw a (lazily loaded, cached) image referenced by `url` into the screen
    /// rect `(dx, dy, dw, dh)` at `alpha`. Backend-agnostic by design: callers
    /// pass a URL string, never a `web-sys` image handle, so the seam stays
    /// swappable. Loading is async — the first call kicks off the fetch and the
    /// draw is skipped until the image is ready, then a redraw is nudged.
    fn draw_image(&self, url: &str, dx: f64, dy: f64, dw: f64, dh: f64, alpha: f64);

    /// Fill a retained [`Geometry`] under `transform` with `color` at `alpha`.
    fn fill_geometry(&self, g: &Geometry, transform: Affine, color: &str, alpha: f64);
    /// Stroke a retained [`Geometry`] under `transform`. `clip`, if given, is
    /// another geometry (under the **same** transform) the stroke is restricted
    /// to the interior of — the hex border outlines clip to their region fill so
    /// adjacent polities abut cleanly instead of double-stroking.
    fn stroke_geometry(
        &self,
        g: &Geometry,
        transform: Affine,
        color: &str,
        stroke: &StrokeStyle,
        clip: Option<&Geometry>,
    );
    /// Push a screen-space clip region (rasterized under the current frame
    /// transform); later draws are restricted to it until [`Canvas::pop_clip`].
    fn push_clip(&self, clip: &Geometry);
    fn pop_clip(&self);
}

thread_local! {
    /// Per-thread cache of lazily-loaded images, keyed by URL. wasm is
    /// single-threaded, so a thread-local `RefCell` is the natural fit.
    static IMAGE_CACHE: RefCell<HashMap<String, HtmlImageElement>> =
        RefCell::new(HashMap::new());
}

/// Tracks the last value pushed to each `CanvasRenderingContext2d` setter so the
/// batched glyph passes (which set font/fill once then loop `fillText`) don't pay
/// a redundant cross-call into JS per glyph when routed through the per-call
/// `Canvas` API. Cleared on every `restore()` (which reverts ctx state, so the
/// cache would otherwise go stale).
#[derive(Default)]
struct DrawState {
    font: Option<String>,
    fill: Option<String>,
    stroke: Option<String>,
    align: Option<&'static str>,
    baseline: Option<&'static str>,
    line_width: Option<f64>,
    cap: Option<&'static str>,
    join: Option<&'static str>,
}

/// Canvas 2D backend over a `CanvasRenderingContext2d`.
pub struct Canvas2d {
    ctx: CanvasRenderingContext2d,
    st: RefCell<DrawState>,
}

impl Canvas2d {
    fn new(ctx: CanvasRenderingContext2d) -> Self {
        Self {
            ctx,
            st: RefCell::new(DrawState::default()),
        }
    }

    /// Acquire the 2D context for `canvas` and prime it for a frame: reset the
    /// transform and scale by the device-pixel-ratio so `Canvas` coordinates are
    /// logical (CSS) pixels and stay crisp on retina. Returns the backend plus
    /// the logical `(width, height, dpr)`.
    pub fn for_frame(canvas: &HtmlCanvasElement) -> Option<(Self, f64, f64, f64)> {
        let ctx = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|o| o.dyn_into::<CanvasRenderingContext2d>().ok())?;
        let (buf_w, css_w) = (canvas.width() as f64, canvas.client_width().max(1) as f64);
        let dpr = buf_w / css_w;
        let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        let _ = ctx.scale(dpr, dpr);
        let h = canvas.client_height().max(1) as f64;
        Some((Self::new(ctx), css_w, h, dpr))
    }

    // ── State setters that skip the JS round-trip when the value is unchanged ──
    fn set_font(&self, font: &str) {
        let mut st = self.st.borrow_mut();
        if st.font.as_deref() != Some(font) {
            self.ctx.set_font(font);
            st.font = Some(font.to_owned());
        }
    }
    fn set_fill(&self, color: &str) {
        let mut st = self.st.borrow_mut();
        if st.fill.as_deref() != Some(color) {
            self.ctx.set_fill_style_str(color);
            st.fill = Some(color.to_owned());
        }
    }
    fn set_stroke(&self, color: &str) {
        let mut st = self.st.borrow_mut();
        if st.stroke.as_deref() != Some(color) {
            self.ctx.set_stroke_style_str(color);
            st.stroke = Some(color.to_owned());
        }
    }
    fn set_align(&self, align: TextAlign) {
        let mut st = self.st.borrow_mut();
        let a = align.as_str();
        if st.align != Some(a) {
            self.ctx.set_text_align(a);
            st.align = Some(a);
        }
    }
    fn set_baseline(&self, baseline: &'static str) {
        let mut st = self.st.borrow_mut();
        if st.baseline != Some(baseline) {
            self.ctx.set_text_baseline(baseline);
            st.baseline = Some(baseline);
        }
    }
    fn set_line_width(&self, w: f64) {
        let mut st = self.st.borrow_mut();
        if st.line_width != Some(w) {
            self.ctx.set_line_width(w);
            st.line_width = Some(w);
        }
    }
    fn set_cap(&self, cap: LineCap) {
        let v = match cap {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
        };
        let mut st = self.st.borrow_mut();
        if st.cap != Some(v) {
            self.ctx.set_line_cap(v);
            st.cap = Some(v);
        }
    }
    fn set_join(&self, join: LineJoin) {
        let v = match join {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
        };
        let mut st = self.st.borrow_mut();
        if st.join != Some(v) {
            self.ctx.set_line_join(v);
            st.join = Some(v);
        }
    }

    /// `save()` keeps the current ctx state, so the dedup cache stays valid.
    fn save(&self) {
        self.ctx.save();
    }
    /// `restore()` reverts ctx state, so the dedup cache must be dropped.
    fn restore(&self) {
        self.ctx.restore();
        *self.st.borrow_mut() = DrawState::default();
    }
}

impl Canvas for Canvas2d {
    fn clear(&self, color: &str, width: f64, height: f64) {
        self.set_fill(color);
        self.ctx.fill_rect(0.0, 0.0, width, height);
    }

    fn fill_circle(&self, x: f64, y: f64, radius: f64, color: &str) {
        self.set_fill(color);
        self.ctx.begin_path();
        // arc() only errors on a non-finite radius, which we never pass.
        let _ = self.ctx.arc(x, y, radius, 0.0, std::f64::consts::TAU);
        self.ctx.fill();
    }

    fn fill_rect(&self, x: f64, y: f64, w: f64, h: f64, color: &str) {
        self.set_fill(color);
        self.ctx.fill_rect(x, y, w, h);
    }

    fn stroke_arc(&self, cx: f64, cy: f64, r: f64, start: f64, end: f64, color: &str, width: f64) {
        self.set_stroke(color);
        self.set_line_width(width);
        self.ctx.begin_path();
        let _ = self.ctx.arc(cx, cy, r, start, end);
        self.ctx.stroke();
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
        self.set_stroke(color);
        self.set_line_width(width);
        self.ctx.begin_path();
        let _ = self
            .ctx
            .ellipse(cx, cy, rx, ry, rot, 0.0, std::f64::consts::TAU);
        self.ctx.stroke();
    }

    fn fill_polygons(&self, polys: &[Vec<(f64, f64)>], color: &str, alpha: f64) {
        self.ctx.set_global_alpha(alpha);
        self.set_fill(color);
        self.ctx.begin_path();
        for poly in polys {
            let Some((first, rest)) = poly.split_first() else {
                continue;
            };
            self.ctx.move_to(first.0, first.1);
            for &(x, y) in rest {
                self.ctx.line_to(x, y);
            }
            self.ctx.close_path();
        }
        self.ctx.fill();
        self.ctx.set_global_alpha(1.0);
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
        if !dash.is_empty() {
            let arr = js_sys::Array::new();
            for &d in dash {
                arr.push(&wasm_bindgen::JsValue::from_f64(d));
            }
            let _ = self.ctx.set_line_dash(&arr);
        }
        self.set_stroke(color);
        self.set_line_width(width);
        self.ctx.begin_path();
        self.ctx.move_to(first.0, first.1);
        for &(x, y) in rest {
            self.ctx.line_to(x, y);
        }
        if close {
            self.ctx.close_path();
        }
        self.ctx.stroke();
        if !dash.is_empty() {
            let _ = self.ctx.set_line_dash(&js_sys::Array::new()); // reset to solid
        }
    }

    fn fill_text(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign) {
        self.set_fill(color);
        self.set_font(font);
        self.set_align(align);
        // Vertically center at y — matches the reference's centered labels, so
        // the parsec-unit Y offsets (starport/UWP/name) place text correctly.
        self.set_baseline("middle");
        let _ = self.ctx.fill_text(text, x, y);
    }

    fn fill_text_top(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign) {
        self.set_fill(color);
        self.set_font(font);
        self.set_align(align);
        self.set_baseline("top");
        let _ = self.ctx.fill_text(text, x, y);
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
        self.save();
        let _ = self.ctx.translate(x, y);
        let _ = self.ctx.rotate(rot);
        let _ = self.ctx.scale(scale_x, scale_y);
        self.set_fill(color);
        self.set_font(font);
        self.set_align(align);
        self.set_baseline("middle");
        if let Some(sh) = shadow {
            self.ctx.set_shadow_color(&sh.color);
            self.ctx.set_shadow_offset_x(sh.dx);
            self.ctx.set_shadow_offset_y(sh.dy);
            self.ctx.set_shadow_blur(sh.blur);
        }
        let _ = self.ctx.fill_text(text, 0.0, 0.0);
        if shadow.is_some() {
            self.ctx.set_shadow_color("rgba(0,0,0,0)");
            self.ctx.set_shadow_offset_x(0.0);
            self.ctx.set_shadow_offset_y(0.0);
            self.ctx.set_shadow_blur(0.0);
        }
        self.restore();
    }

    fn draw_image(&self, url: &str, dx: f64, dy: f64, dw: f64, dh: f64, alpha: f64) {
        if alpha <= 0.0 {
            return;
        }
        IMAGE_CACHE.with(|cache| {
            let img = cache.borrow().get(url).cloned();
            let img = match img {
                Some(img) => img,
                None => {
                    // First time we've seen this URL: create the element, start
                    // the load, and nudge a redraw once it finishes so the
                    // freshly-decoded image paints on the next frame. The app
                    // already re-renders on window "resize", so reuse that.
                    let img = HtmlImageElement::new().unwrap();
                    let onload = Closure::<dyn FnMut()>::new(move || {
                        if let Some(win) = web_sys::window() {
                            if let Ok(ev) = web_sys::Event::new("resize") {
                                let _ = win.dispatch_event(&ev);
                            }
                        }
                    });
                    img.set_onload(Some(onload.as_ref().unchecked_ref()));
                    onload.forget(); // keep the closure alive for the image's lifetime
                    img.set_src(url);
                    cache.borrow_mut().insert(url.to_string(), img.clone());
                    img
                }
            };
            // Only draw once decoded; until then the load is in flight.
            if img.complete() && img.natural_width() > 0 {
                self.save();
                self.ctx.set_global_alpha(alpha);
                let _ = self
                    .ctx
                    .draw_image_with_html_image_element_and_dw_and_dh(&img, dx, dy, dw, dh);
                self.ctx.set_global_alpha(1.0);
                self.restore();
            }
        });
    }

    fn fill_geometry(&self, g: &Geometry, transform: Affine, color: &str, alpha: f64) {
        self.save();
        let _ = self.ctx.set_transform(
            transform.a,
            transform.b,
            transform.c,
            transform.d,
            transform.e,
            transform.f,
        );
        self.set_fill(color);
        if alpha != 1.0 {
            self.ctx.set_global_alpha(alpha);
        }
        self.ctx.fill_with_path_2d(g.path());
        if alpha != 1.0 {
            self.ctx.set_global_alpha(1.0);
        }
        self.restore();
    }

    fn stroke_geometry(
        &self,
        g: &Geometry,
        transform: Affine,
        color: &str,
        stroke: &StrokeStyle,
        clip: Option<&Geometry>,
    ) {
        self.save();
        let _ = self.ctx.set_transform(
            transform.a,
            transform.b,
            transform.c,
            transform.d,
            transform.e,
            transform.f,
        );
        if let Some(c) = clip {
            self.ctx.clip_with_path_2d(c.path());
        }
        self.set_stroke(color);
        self.set_line_width(stroke.width);
        self.set_cap(stroke.cap);
        self.set_join(stroke.join);
        self.ctx.stroke_with_path(g.path());
        self.restore();
    }

    fn push_clip(&self, clip: &Geometry) {
        self.save();
        self.ctx.clip_with_path_2d(clip.path());
    }

    fn pop_clip(&self) {
        self.restore();
    }
}
