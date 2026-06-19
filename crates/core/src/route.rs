//! Jump-route finding over a set of worlds — a pure, I/O-free A* search.
//!
//! Ported from the reference `server/api/RouteHandler.cs` (the cross-sector
//! jump-route API) and its underlying `PathFinder`. Nodes are worlds keyed by
//! absolute [`Coord`]; an edge connects two worlds whose hex distance ≤ the
//! ship's `jump` rating, the obstacle is "no world in this hex". The result is
//! the cheapest sequence of worlds from start to end.
//!
//! **Cost model (matches `RouteHandler.cs`):**
//! - *Edge weight* = `1 + hex_distance / 36` — the dominant term is `1` per
//!   jump, so the route **minimizes the number of jumps**; the small
//!   `hex_distance/36` term (always `< 1` for any legal jump ≤ 12 pc) breaks
//!   ties toward shorter total parsecs / less fuel. This is the reference's
//!   `PathFinder.IMap.EdgeWeight`.
//! - *Heuristic* = `ceil(hex_distance(node, goal) / jump)` — an admissible lower
//!   bound on remaining jumps (the reference's `CostEstimate`), so A* stays
//!   optimal under the jump-count objective.
//!
//! The reference also supports filters (avoid red zones, wilderness-refuelling
//! only, Imperial-only, allow anomalies) by *excluding* worlds from the
//! neighbor set rather than weighting them. We keep the same approach via
//! [`RouteOptions`]; the cost stays jump-count-first exactly as the reference's
//! `EdgeWeight` does (its red/no-fuel weighting is a documented TODO, not live).

use crate::astrometrics::Coord;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// A candidate world the route may pass through. Pure data — the backend builds
/// these from `res/` and reads names/hex back out of the returned indices.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteWorld {
    /// Absolute world coordinate (parsec offsets) — the node key.
    pub coord: Coord,
    /// Is this a Red travel zone? (`zone == "R"`.)
    pub red: bool,
    /// An "anomaly" / deep-space object (no real UWP) — skipped unless allowed.
    pub anomaly: bool,
    /// Allegiance is the default Imperial allegiance (for the Imperial-only filter).
    pub imperial: bool,
    /// Has a gas giant or surface water (for the wilderness-refuelling filter).
    pub refuel: bool,
}

/// Neighbor-filtering options, mirroring `RouteHandler`'s query flags. All
/// off by default = the plain shortest jump route. Filters *exclude* worlds
/// from being used as intermediate stops (the start/end are always allowed).
#[derive(Debug, Clone, Copy, Default)]
pub struct RouteOptions {
    pub avoid_red: bool,
    pub require_refuel: bool,
    pub imperial_only: bool,
    pub allow_anomalies: bool,
}

/// Hex distance divisor for the tie-break fuel term — straight from
/// `RouteHandler.EdgeWeight` (`1 + HexDistance / 36.0`).
const FUEL_DIVISOR: f64 = 36.0;

/// Cost of a single jump between two worlds: `1` (the jump) plus a small fuel
/// term so longer legs are slightly dispreferred among equal-jump-count routes.
fn edge_weight(a: Coord, b: Coord) -> f64 {
    1.0 + (a.hex_distance(b) as f64 / FUEL_DIVISOR)
}

/// Admissible heuristic: a lower bound on the jumps still needed to reach the
/// goal (`RouteHandler.CostEstimate`).
fn heuristic(a: Coord, goal: Coord, jump: i32) -> f64 {
    (a.hex_distance(goal) as f64 / jump as f64).ceil()
}

/// An entry in the A* open set. Ordered by `f = g + h` ascending (so the
/// `BinaryHeap`, a max-heap, is reversed to pop the lowest `f`).
struct Frontier {
    f: f64,
    idx: usize,
}
impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f
    }
}
impl Eq for Frontier {}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse so the smallest `f` is "greatest" → pops first. NaN can't
        // occur (all costs finite), so `total_cmp` is safe and total.
        other.f.total_cmp(&self.f)
    }
}

/// Find the cheapest jump route from `start` to `end` over `worlds`.
///
/// `worlds[start]`/`worlds[end]` are the endpoints (indices into the slice);
/// `jump` is the drive rating in parsecs. Returns the ordered list of world
/// indices (including both endpoints) for the optimal route, or `None` if no
/// route exists. Endpoints are exempt from the [`RouteOptions`] filters (you
/// can always depart from and arrive at your chosen worlds).
pub fn find_route(
    worlds: &[RouteWorld],
    start: usize,
    end: usize,
    jump: i32,
    opts: RouteOptions,
) -> Option<Vec<usize>> {
    if start >= worlds.len() || end >= worlds.len() || jump < 1 {
        return None;
    }
    if start == end {
        return Some(vec![start]);
    }
    let goal = worlds[end].coord;

    // Spatial bucket index (coord → world index) so neighbor lookups scan only
    // the local cells, not every world — the reference's `HexSelector` does the
    // same via the sector map. Bucket by jump-sized cells in raw hex space.
    let cell = jump.max(1);
    let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, w) in worlds.iter().enumerate() {
        let key = (w.coord.x.div_euclid(cell), w.coord.y.div_euclid(cell));
        buckets.entry(key).or_default().push(i);
    }

    // Is world `i` usable as an *intermediate* stop under the active filters?
    // Endpoints bypass this (handled at call sites).
    let allowed = |i: usize| -> bool {
        let w = &worlds[i];
        if !opts.allow_anomalies && w.anomaly {
            return false;
        }
        if opts.avoid_red && w.red {
            return false;
        }
        if opts.require_refuel && !w.refuel {
            return false;
        }
        if opts.imperial_only && !w.imperial {
            return false;
        }
        true
    };

    let n = worlds.len();
    let mut g = vec![f64::INFINITY; n]; // best known cost from start
    let mut came_from = vec![usize::MAX; n];
    let mut closed = vec![false; n];
    let mut open = BinaryHeap::new();

    g[start] = 0.0;
    open.push(Frontier {
        f: heuristic(worlds[start].coord, goal, jump),
        idx: start,
    });

    while let Some(Frontier { idx: current, .. }) = open.pop() {
        if current == end {
            // Reconstruct the path by following parent links.
            let mut path = vec![end];
            let mut node = end;
            while node != start {
                node = came_from[node];
                path.push(node);
            }
            path.reverse();
            return Some(path);
        }
        if closed[current] {
            continue; // stale heap entry (we found a cheaper route already)
        }
        closed[current] = true;

        let c = worlds[current].coord;
        let (kx, ky) = (c.x.div_euclid(cell), c.y.div_euclid(cell));
        // Candidate cells: the current cell and its 8 neighbors cover every hex
        // within `jump` (a jump-sized cell + one ring).
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(bucket) = buckets.get(&(kx + dx, ky + dy)) else {
                    continue;
                };
                for &nb in bucket {
                    if nb == current || closed[nb] {
                        continue;
                    }
                    if c.hex_distance(worlds[nb].coord) > jump {
                        continue; // out of jump range
                    }
                    // Endpoints skip the filters; intermediates must pass them.
                    if nb != end && !allowed(nb) {
                        continue;
                    }
                    let tentative = g[current] + edge_weight(c, worlds[nb].coord);
                    if tentative < g[nb] {
                        g[nb] = tentative;
                        came_from[nb] = current;
                        open.push(Frontier {
                            f: tentative + heuristic(worlds[nb].coord, goal, jump),
                            idx: nb,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Total path length in parsecs (sum of per-jump hex distances) for a route.
pub fn path_parsecs(worlds: &[RouteWorld], path: &[usize]) -> i32 {
    path.windows(2)
        .map(|w| worlds[w[0]].coord.hex_distance(worlds[w[1]].coord))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(x: i32, y: i32) -> RouteWorld {
        RouteWorld {
            coord: Coord::new(x, y),
            red: false,
            anomaly: false,
            imperial: true,
            refuel: true,
        }
    }

    // A line of worlds one parsec apart. With jump-1 the route must visit every
    // world; the cheapest path is the straight chain.
    #[test]
    fn straight_chain_jump1() {
        let worlds = vec![w(1, 1), w(1, 2), w(1, 3), w(1, 4)];
        let path = find_route(&worlds, 0, 3, 1, RouteOptions::default()).expect("route exists");
        assert_eq!(path, vec![0, 1, 2, 3]);
        assert_eq!(path_parsecs(&worlds, &path), 3);
    }

    // With jump-2 the same line can skip every other world: fewer jumps wins.
    #[test]
    fn jump2_skips_worlds() {
        let worlds = vec![w(1, 1), w(1, 2), w(1, 3), w(1, 4), w(1, 5)];
        let path = find_route(&worlds, 0, 4, 2, RouteOptions::default()).expect("route exists");
        // 4 parsecs / jump 2 = 2 jumps minimum: 0 -> 2 -> 4.
        assert_eq!(path.len(), 3);
        assert_eq!(path.first(), Some(&0));
        assert_eq!(path.last(), Some(&4));
        assert_eq!(path.len() - 1, 2, "should take exactly 2 jumps");
    }

    // A gap wider than the jump range = no route.
    #[test]
    fn no_route_across_gap() {
        // Two clusters 5 parsecs apart, jump 2 can't bridge them.
        let worlds = vec![w(1, 1), w(1, 2), w(1, 7), w(1, 8)];
        assert!(find_route(&worlds, 0, 3, 2, RouteOptions::default()).is_none());
    }

    // Prefers the shorter total-parsec route among equal jump counts (the fuel
    // tie-break term). Two 1-jump options to the goal: pick the closer one is
    // moot for a single jump, so test a 2-jump fork.
    #[test]
    fn ties_break_on_distance() {
        // 0 at (1,1), goal 3 at (1,5). Two intermediates both reachable in the
        // same jump count: 1 at (1,3) (legs 2+2=4pc) vs 2 at (2,2)-ish longer.
        let worlds = vec![
            w(1, 1), // 0 start
            w(1, 3), // 1 close intermediate (dist 2 then 2)
            w(3, 2), // 2 far intermediate (longer legs)
            w(1, 5), // 3 goal
        ];
        let path = find_route(&worlds, 0, 3, 2, RouteOptions::default()).expect("route");
        assert_eq!(path.len() - 1, 2, "2 jumps");
        assert_eq!(path[1], 1, "should route via the closer intermediate");
    }

    // A red-zone intermediate is avoided when `avoid_red` is set, even if it
    // forces a longer route; endpoints are exempt.
    #[test]
    fn avoid_red_reroutes() {
        let mut worlds = vec![w(1, 1), w(1, 3), w(1, 5)];
        worlds[1].red = true; // the only jump-2 stepping stone is red
                              // Without avoidance, 0 -> 1 -> 2 works.
        assert!(find_route(&worlds, 0, 2, 2, RouteOptions::default()).is_some());
        // With avoidance and no alternative, no route.
        let opts = RouteOptions {
            avoid_red: true,
            ..Default::default()
        };
        assert!(find_route(&worlds, 0, 2, 2, opts).is_none());
        // But a red *endpoint* is still reachable.
        let mut worlds2 = vec![w(1, 1), w(1, 2)];
        worlds2[1].red = true;
        assert!(find_route(&worlds2, 0, 1, 1, opts).is_some());
    }

    #[test]
    fn start_equals_end() {
        let worlds = vec![w(1, 1)];
        let path = find_route(&worlds, 0, 0, 2, RouteOptions::default()).unwrap();
        assert_eq!(path, vec![0]);
        assert_eq!(path_parsecs(&worlds, &path), 0);
    }
}
