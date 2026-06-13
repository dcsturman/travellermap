//! The `Canvas` graphics trait and its Canvas 2D (`web-sys`) backend.
//!
//! This is the swappable seam (mirrors the reference `AbstractGraphics`): all
//! map-drawing logic in `render.rs` calls `trait Canvas`, never `web-sys`
//! directly. Today the only impl is [`Canvas2d`] (browser-native rasterization
//! via `CanvasRenderingContext2d`); a future `WgpuCanvas` can drop in without
//! touching scene logic. See PORT_PLAN.md "Rendering" decision.

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlImageElement};

#[derive(Clone, Copy)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Immediate-mode drawing surface. Coordinates are screen pixels; colors are
/// any CSS color string.
pub trait Canvas {
    fn clear(&self, color: &str, width: f64, height: f64);
    fn fill_circle(&self, x: f64, y: f64, radius: f64, color: &str);
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
    fn fill_text(&self, text: &str, x: f64, y: f64, color: &str, font: &str, align: TextAlign);
    /// Centered text rotated by `rot` radians about `(x, y)`, with an
    /// independent horizontal scale (for the squished diagonal sector/subsector
    /// watermark labels).
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
    );
    /// Draw a (lazily loaded, cached) image referenced by `url` into the screen
    /// rect `(dx, dy, dw, dh)` at `alpha`. Backend-agnostic by design: callers
    /// pass a URL string, never a `web-sys` image handle, so the seam stays
    /// swappable. Loading is async — the first call kicks off the fetch and the
    /// draw is skipped until the image is ready, then a redraw is nudged.
    fn draw_image(&self, url: &str, dx: f64, dy: f64, dw: f64, dh: f64, alpha: f64);
}

thread_local! {
    /// Per-thread cache of lazily-loaded images, keyed by URL. wasm is
    /// single-threaded, so a thread-local `RefCell` is the natural fit.
    static IMAGE_CACHE: RefCell<HashMap<String, HtmlImageElement>> =
        RefCell::new(HashMap::new());
}

/// Canvas 2D backend over a `CanvasRenderingContext2d`.
pub struct Canvas2d {
    pub ctx: CanvasRenderingContext2d,
}

impl Canvas for Canvas2d {
    fn clear(&self, color: &str, width: f64, height: f64) {
        self.ctx.set_fill_style_str(color);
        self.ctx.fill_rect(0.0, 0.0, width, height);
    }

    fn fill_circle(&self, x: f64, y: f64, radius: f64, color: &str) {
        self.ctx.set_fill_style_str(color);
        self.ctx.begin_path();
        // arc() only errors on a non-finite radius, which we never pass.
        let _ = self.ctx.arc(x, y, radius, 0.0, std::f64::consts::TAU);
        self.ctx.fill();
    }

    fn fill_polygons(&self, polys: &[Vec<(f64, f64)>], color: &str, alpha: f64) {
        self.ctx.set_global_alpha(alpha);
        self.ctx.set_fill_style_str(color);
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
        self.ctx.set_stroke_style_str(color);
        self.ctx.set_line_width(width);
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
        self.ctx.set_fill_style_str(color);
        self.ctx.set_font(font);
        self.ctx.set_text_align(match align {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        });
        // Vertically center at y — matches the reference's centered labels, so
        // the parsec-unit Y offsets (starport/UWP/name) place text correctly.
        self.ctx.set_text_baseline("middle");
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
    ) {
        self.ctx.save();
        let _ = self.ctx.translate(x, y);
        let _ = self.ctx.rotate(rot);
        let _ = self.ctx.scale(scale_x, 1.0);
        self.ctx.set_fill_style_str(color);
        self.ctx.set_font(font);
        self.ctx.set_text_align("center");
        self.ctx.set_text_baseline("middle");
        let _ = self.ctx.fill_text(text, 0.0, 0.0);
        self.ctx.restore();
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
                    cache
                        .borrow_mut()
                        .insert(url.to_string(), img.clone());
                    img
                }
            };
            // Only draw once decoded; until then the load is in flight.
            if img.complete() && img.natural_width() > 0 {
                self.ctx.save();
                self.ctx.set_global_alpha(alpha);
                let _ = self
                    .ctx
                    .draw_image_with_html_image_element_and_dw_and_dh(&img, dx, dy, dw, dh);
                self.ctx.set_global_alpha(1.0);
                self.ctx.restore();
            }
        });
    }
}
