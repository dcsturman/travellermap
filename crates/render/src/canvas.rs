//! The `Canvas` graphics trait, its geometry types, and a backend-neutral
//! retained-path representation.
//!
//! This is the swappable seam (mirrors the reference `AbstractGraphics`): all
//! map-drawing logic in `render/` calls `trait Canvas`, never a concrete
//! backend. Two impls exist: the frontend's `Canvas2d` (browser
//! `CanvasRenderingContext2d`) and the backend's `SvgCanvas` (emits SVG). This
//! crate stays web-sys-free so it compiles native *and* wasm.
//!
//! The seam has three layers of primitive:
//! - **Immediate shapes/text** — `fill_circle`/`fill_rect`/`fill_text`/… draw
//!   once in screen pixels.
//! - **Retained [`Geometry`]** — a path built once (in any coordinate space) via
//!   [`PathBuilder`] and re-`fill_geometry`/`stroke_geometry`d each frame under
//!   an [`Affine`] transform. This is the hot, cached map layer (borders, the hex
//!   grid, world dots): a pan re-transforms instead of re-emitting the path. A
//!   [`Geometry`] is just a list of [`PathCmd`]s; each backend interprets it
//!   (the Canvas2d backend replays it into a `Path2d`, the SVG backend into a
//!   `d` attribute) — the render passes never see either.
//! - **Clip** — `push_clip`/`pop_clip` (screen-space) and the `clip` argument of
//!   `stroke_geometry` (world-space, paired with the stroked geometry).

use std::cell::RefCell;

#[derive(Clone, Copy)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl TextAlign {
    /// Canvas 2D `textAlign` value.
    pub fn as_str(self) -> &'static str {
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
    /// Apply to a point.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
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

/// One path command, mirroring the subset of the Canvas 2D `Path2d` API the map
/// needs. A [`Geometry`] is a flat list of these; backends replay them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    /// `arc(cx, cy, r, start, end)` — same semantics as Canvas 2D `arc`
    /// (clockwise, connected from the current point).
    Arc {
        cx: f64,
        cy: f64,
        r: f64,
        start: f64,
        end: f64,
    },
    Bezier {
        c1x: f64,
        c1y: f64,
        c2x: f64,
        c2y: f64,
        x: f64,
        y: f64,
    },
    Close,
}

/// Retained path geometry: built once via [`PathBuilder`], drawn many times. The
/// render passes treat it as opaque; a backend reads [`Geometry::cmds`] to
/// rasterize it (Canvas2d → `Path2d`, SVG → `d` string).
pub struct Geometry {
    cmds: Vec<PathCmd>,
}

impl Geometry {
    /// The path commands, in order — for a backend to replay.
    pub fn cmds(&self) -> &[PathCmd] {
        &self.cmds
    }
}

/// Accumulates a [`Geometry`] from path commands. Mirrors the subset of the
/// `Path2d` API the map needs, in backend-neutral terms. Methods take `&self`
/// (interior-mutable) to match the call sites the Canvas2d backend grew up with.
pub struct PathBuilder {
    cmds: RefCell<Vec<PathCmd>>,
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBuilder {
    pub fn new() -> Self {
        Self {
            cmds: RefCell::new(Vec::new()),
        }
    }
    pub fn move_to(&self, x: f64, y: f64) {
        self.cmds.borrow_mut().push(PathCmd::MoveTo(x, y));
    }
    pub fn line_to(&self, x: f64, y: f64) {
        self.cmds.borrow_mut().push(PathCmd::LineTo(x, y));
    }
    pub fn close(&self) {
        self.cmds.borrow_mut().push(PathCmd::Close);
    }
    /// Arc (full circle: `move_to(cx+r, cy)` first, then `arc(cx,cy,r,0,TAU)`).
    pub fn arc(&self, cx: f64, cy: f64, r: f64, start: f64, end: f64) {
        self.cmds.borrow_mut().push(PathCmd::Arc {
            cx,
            cy,
            r,
            start,
            end,
        });
    }
    pub fn bezier_to(&self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        self.cmds.borrow_mut().push(PathCmd::Bezier {
            c1x,
            c1y,
            c2x,
            c2y,
            x,
            y,
        });
    }
    /// Append another (already-built) geometry's commands — the per-frame combine
    /// of cached per-sector geometries into per-group geometries.
    pub fn add(&self, g: &Geometry) {
        self.cmds.borrow_mut().extend_from_slice(g.cmds());
    }
    pub fn finish(self) -> Geometry {
        Geometry {
            cmds: self.cmds.into_inner(),
        }
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
