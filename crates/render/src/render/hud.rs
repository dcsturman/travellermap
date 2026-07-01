//! Profiling overlay: per-layer frame timings, bottom-left (debug toggle).

use crate::canvas::{Canvas, TextAlign};

use super::borders::BORDER_STATS;

/// Profiling overlay: per-layer milliseconds + total/fps, bottom-left. Layers
/// over 4 ms are flagged red. Toggle via the settings menu ("Frame Timing").
pub(crate) fn draw_perf_hud(
    c: &impl Canvas,
    _w: f64,
    h: f64,
    marks: &[(&str, f64)],
    n_sectors: usize,
    scale: f64,
) {
    let total: f64 = marks.iter().map(|(_, ms)| ms).sum();
    let mono = "12px ui-monospace, Menlo, monospace";
    let line_h = 15.0;
    let x = 12.0;
    let box_h = (marks.len() as f64 + 3.0) * line_h + 14.0;
    // Clear the fixed credits footer (a ~7px red bar + up to 3 lines of ~14px
    // credit text + padding ≈ 86px). Keep the box's bottom edge above it.
    let footer_clearance = 96.0;
    let top = h - box_h - footer_clearance;
    c.fill_polygons(
        &[vec![
            (x - 6.0, top - 6.0),
            (x + 204.0, top - 6.0),
            (x + 204.0, top + box_h - 6.0),
            (x - 6.0, top + box_h - 6.0),
        ]],
        "#0b0e16",
        0.84,
    );
    let mut y = top + line_h;
    let fps = if total > 0.0 { 1000.0 / total } else { 0.0 };
    c.fill_text(
        &format!("FRAME  {total:5.1} ms   {fps:3.0} fps"),
        x,
        y,
        "#9ef0a0",
        mono,
        TextAlign::Left,
    );
    y += line_h;
    c.fill_text(
        &format!("scale {scale:6.1}   sectors {n_sectors}"),
        x,
        y,
        "#aab3c8",
        mono,
        TextAlign::Left,
    );
    y += line_h;
    for (label, ms) in marks {
        let col = if *ms > 4.0 { "#ffb0b0" } else { "#c9d2e4" };
        c.fill_text(
            &format!("{label:<14}{ms:6.1}"),
            x,
            y,
            col,
            mono,
            TextAlign::Left,
        );
        y += line_h;
    }
    // Border cache detail: is this frame a rebuild (expensive) or cached redraw?
    let bs = BORDER_STATS.with(|s| s.get());
    let (bline, bcol) = if bs.rebuilt {
        (
            format!(
                "↻ BUILD {}grp {}hex {:.1}ms",
                bs.groups, bs.hexes, bs.build_ms
            ),
            "#ffd0a0",
        )
    } else {
        (format!("border cached {}grp", bs.groups), "#9aa3b8")
    };
    c.fill_text(&bline, x, y, bcol, mono, TextAlign::Left);
}
