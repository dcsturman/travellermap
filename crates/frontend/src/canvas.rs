//! The browser Canvas 2D backend (`web-sys`) for the shared render passes.
//!
//! The `Canvas` trait, its geometry types, and the backend-neutral retained
//! [`Geometry`] (a list of [`PathCmd`]s) live in the web-free `tmap-render`
//! crate; this module is the one concrete impl for the browser, over a
//! `CanvasRenderingContext2d`. A [`Geometry`] is replayed into a `Path2d` on
//! demand (see [`to_path2d`]); the render passes never see either.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, Path2d};

// Re-export the shared graphics seam so existing `crate::canvas::…` paths (and
// `main.rs`) keep resolving after the move into `tmap-render`.
pub use tmap_render::canvas::{
    Affine, Canvas, Geometry, LineCap, LineJoin, PathCmd, Shadow, StrokeStyle, TextAlign,
};

thread_local! {
    /// Per-thread cache of lazily-loaded images, keyed by URL. wasm is
    /// single-threaded, so a thread-local `RefCell` is the natural fit.
    static IMAGE_CACHE: RefCell<HashMap<String, HtmlImageElement>> =
        RefCell::new(HashMap::new());
}

/// Replay a backend-neutral [`Geometry`] into a browser `Path2d` for filling,
/// stroking, or clipping.
///
/// The materialized `Path2d` is **memoized on the `Geometry`** (via its
/// `backend_cache` slot) and reused across frames. This matters for the cached
/// passes (borders, grid, world dots): their geometry is rebuilt only when the
/// on-screen set changes, but is drawn every frame, and the fill geometry runs
/// to tens of thousands of vertices. Without memoization each frame replayed
/// every `move_to`/`line_to` across the WASM↔JS boundary (and the hex-border
/// pass does it twice — fill + clip), which was a large per-frame cost. The
/// stored handle is an `Rc<Path2d>`; cloning it is a refcount bump, and the
/// `Geometry` outlives the frame in its cache, so the conversion happens once
/// per rebuild instead of once per draw.
fn to_path2d(g: &Geometry) -> Rc<Path2d> {
    if let Some(handle) = g.backend_cache() {
        if let Ok(path) = handle.downcast::<Path2d>() {
            return path;
        }
    }
    // Path2d::new only fails on OOM, which we treat as fatal elsewhere too.
    let path = Path2d::new().unwrap();
    for cmd in g.cmds() {
        match *cmd {
            PathCmd::MoveTo(x, y) => path.move_to(x, y),
            PathCmd::LineTo(x, y) => path.line_to(x, y),
            // arc only errors on a non-finite radius, which callers never pass.
            PathCmd::Arc {
                cx,
                cy,
                r,
                start,
                end,
            } => {
                let _ = path.arc(cx, cy, r, start, end);
            }
            PathCmd::Bezier {
                c1x,
                c1y,
                c2x,
                c2y,
                x,
                y,
            } => path.bezier_curve_to(c1x, c1y, c2x, c2y, x, y),
            PathCmd::Close => path.close_path(),
        }
    }
    let path = Rc::new(path);
    g.set_backend_cache(path.clone());
    path
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
        self.ctx.fill_with_path_2d(&to_path2d(g));
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
            self.ctx.clip_with_path_2d(&to_path2d(c));
        }
        self.set_stroke(color);
        self.set_line_width(stroke.width);
        self.set_cap(stroke.cap);
        self.set_join(stroke.join);
        self.ctx.stroke_with_path(&to_path2d(g));
        self.restore();
    }

    fn push_clip(&self, clip: &Geometry) {
        self.save();
        self.ctx.clip_with_path_2d(&to_path2d(clip));
    }

    fn pop_clip(&self) {
        self.restore();
    }
}
