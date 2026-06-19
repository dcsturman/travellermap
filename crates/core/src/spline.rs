//! Cardinal-spline → cubic-Bézier conversion, ported **verbatim** from the
//! reference `server/graphics/SVGGraphics.cs` `ToSVG` (the platform-independent
//! curve math behind GDI+'s `DrawCurve`/`DrawClosedCurve`). The reference draws
//! curved micro-borders with this at `tension = 0.6` (stroke) / `0.5` (fill); the
//! browser renderer has no native cardinal-spline primitive, so we expand to
//! Béziers here and feed them to `Path2d::bezier_curve_to`.
//!
//! Pure geometry, I/O-free → lives in `tmap-core` (shared, native + wasm).

/// One cubic-Bézier segment: the two control points then the end point. The start
/// point is the previous segment's end (or the spline's first point for the first
/// segment), so a caller does `move_to(points[0])` then one `bezier_curve_to` per
/// returned triple.
pub type Bezier = [(f64, f64); 3];

/// Expand a cardinal spline through `points` into cubic-Bézier segments.
///
/// Verbatim port of `SVGGraphics.ToSVG(points, tension, closed)`: the tangent at
/// each point is `(P[i+1] − P[i−1]) / (tension + 1)` (one-sided at the ends of an
/// open spline; wrapping for a closed one), and each segment's control handles sit
/// at `±tangent/3`. For `closed`, a final segment wraps back to `points[0]` (the
/// caller should `close_path()` after). Returns empty for fewer than two points.
pub fn cardinal_spline_beziers(points: &[(f64, f64)], tension: f64, closed: bool) -> Vec<Bezier> {
    let n = points.len();
    if n < 2 {
        return Vec::new();
    }
    let a = tension + 1.0;
    let deriv = |i: usize| -> (f64, f64) {
        if closed {
            let j = (i + 1) % n;
            let k = if i > 0 { i - 1 } else { n - 1 };
            (
                (points[j].0 - points[k].0) / a,
                (points[j].1 - points[k].1) / a,
            )
        } else if i == 0 {
            (
                (points[1].0 - points[0].0) / a,
                (points[1].1 - points[0].1) / a,
            )
        } else if i == n - 1 {
            (
                (points[i].0 - points[i - 1].0) / a,
                (points[i].1 - points[i - 1].1) / a,
            )
        } else {
            (
                (points[i + 1].0 - points[i - 1].0) / a,
                (points[i + 1].1 - points[i - 1].1) / a,
            )
        }
    };
    let seg =
        |last: (f64, f64), lastd: (f64, f64), point: (f64, f64), pointd: (f64, f64)| -> Bezier {
            [
                (last.0 + lastd.0 / 3.0, last.1 + lastd.1 / 3.0),
                (point.0 - pointd.0 / 3.0, point.1 - pointd.1 / 3.0),
                (point.0, point.1),
            ]
        };
    let mut out = Vec::with_capacity(n);
    let mut last = points[0];
    let mut lastd = deriv(0);
    for (i, &point) in points.iter().enumerate().skip(1) {
        let pointd = deriv(i);
        out.push(seg(last, lastd, point, pointd));
        last = point;
        lastd = pointd;
    }
    if closed {
        out.push(seg(last, lastd, points[0], deriv(0)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: (f64, f64), b: (f64, f64)) {
        assert!(
            (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
            "{a:?} vs {b:?}"
        );
    }

    #[test]
    fn two_points_open() {
        // tension 0.5 → a = 1.5; both tangents = (P1−P0)/a = (2, 0).
        // c1 = P0 + d/3 = (2/3, 0); c2 = P1 − d/3 = (3 − 2/3, 0); end = P1.
        let b = cardinal_spline_beziers(&[(0.0, 0.0), (3.0, 0.0)], 0.5, false);
        assert_eq!(b.len(), 1);
        approx(b[0][0], (2.0 / 3.0, 0.0));
        approx(b[0][1], (3.0 - 2.0 / 3.0, 0.0));
        approx(b[0][2], (3.0, 0.0));
    }

    #[test]
    fn closed_triangle_wraps() {
        // A closed 3-point spline emits 3 segments (one wrapping back to P0).
        let pts = [(0.0, 0.0), (4.0, 0.0), (2.0, 3.0)];
        let b = cardinal_spline_beziers(&pts, 0.6, true);
        assert_eq!(b.len(), 3);
        // Each segment ends on the next point; the last wraps to points[0].
        approx(b[0][2], pts[1]);
        approx(b[1][2], pts[2]);
        approx(b[2][2], pts[0]);
        // Closed tangent at P1 = (P2 − P0)/a = ((2−0)/1.6, (3−0)/1.6).
        let a = 1.6;
        approx(
            b[0][1],
            (pts[1].0 - (2.0 / a) / 3.0, pts[1].1 - (3.0 / a) / 3.0),
        );
    }

    #[test]
    fn degenerate_inputs() {
        assert!(cardinal_spline_beziers(&[], 0.6, false).is_empty());
        assert!(cardinal_spline_beziers(&[(1.0, 1.0)], 0.6, true).is_empty());
    }
}
