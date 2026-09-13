//! Plain 2D geometry helpers `kr_buildings` needs and no other module in
//! this crate already exposes: polygon area/point-in-polygon for footprint
//! rasterization, point-to-polyline distance for the road-occupancy clip
//! (SPEC_Ingest.md §4.2 step 2).

/// Shoelace formula, signed (CCW positive) -- used only for its absolute
/// value (ring size, to pick the outer ring out of a multi-part polygon
/// shape), so the sign convention itself is never relied on.
pub(super) fn ring_area_abs(ring: &[(f64, f64)]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..ring.len() {
        let (x0, y0) = ring[i];
        let (x1, y1) = ring[(i + 1) % ring.len()];
        sum += x0 * y1 - x1 * y0;
    }
    (sum / 2.0).abs()
}

/// A polygon shapefile record's rings, largest-by-area first -- the outer
/// boundary in ESRI's convention, though this crate has no test data with
/// holes to verify winding order against, so picking by area rather than by
/// winding direction is the safer of the two (SPEC_Ingest.md §4 doesn't
/// mention holes at all; the disclosed simplification is dropping every ring
/// but this one, i.e. any hole is filled in rather than left open).
pub(super) fn largest_ring(rings: &[Vec<(f64, f64)>]) -> Option<&Vec<(f64, f64)>> {
    rings.iter().max_by(|a, b| ring_area_abs(a).partial_cmp(&ring_area_abs(b)).unwrap())
}

/// Standard ray-casting point-in-polygon test (even-odd rule), used at each
/// candidate cell's center to rasterize a footprint onto the block grid.
pub(super) fn point_in_polygon(pt: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let (px, py) = pt;
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) {
            let x_intersect = xi + (py - yi) * (xj - xi) / (yj - yi);
            if px < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

pub(super) fn polygon_bbox(poly: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::MAX;
    let mut max_x = f64::MIN;
    let mut min_y = f64::MAX;
    let mut max_y = f64::MIN;
    for &(x, y) in poly {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (min_x, min_y, max_x, max_y)
}

/// Shortest distance from `p` to the segment `a`-`b`.
pub(super) fn point_segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-9 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Shortest distance from `p` to any segment of the polyline `points`.
pub(super) fn point_polyline_distance(p: (f64, f64), points: &[(i32, i32)]) -> f64 {
    if points.len() < 2 {
        return points
            .first()
            .map(|&(x, z)| point_segment_distance(p, (x as f64, z as f64), (x as f64, z as f64)))
            .unwrap_or(f64::MAX);
    }
    let mut best = f64::MAX;
    for w in points.windows(2) {
        let a = (w[0].0 as f64, w[0].1 as f64);
        let b = (w[1].0 as f64, w[1].1 as f64);
        best = best.min(point_segment_distance(p, a, b));
    }
    best
}

/// FNV-1a over a building's own identifier -- SPEC_BuildingType.md §7's
/// "재질 변주, 셔터 여부, 간판 색, 돌출 간판 배치는 모두 건물 ID 해시로
/// 결정한다" applied literally: same id -> same hash -> same choices, every
/// run, with no dependency on placement order or Arnis's own RNG.
pub(super) fn building_hash(id: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_polygon_finds_interior_and_exterior_points() {
        let square = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(point_in_polygon((5.0, 5.0), &square));
        assert!(!point_in_polygon((15.0, 5.0), &square));
        assert!(!point_in_polygon((-1.0, 5.0), &square));
    }

    #[test]
    fn ring_area_matches_known_rectangle() {
        let rect = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 3.0)];
        assert!((ring_area_abs(&rect) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn largest_ring_picks_the_bigger_one() {
        let small = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let big = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let rings = vec![small.clone(), big.clone()];
        assert_eq!(largest_ring(&rings), Some(&big));
    }

    #[test]
    fn point_segment_distance_is_zero_on_the_segment() {
        let d = point_segment_distance((5.0, 0.0), (0.0, 0.0), (10.0, 0.0));
        assert!(d < 1e-9);
    }

    #[test]
    fn building_hash_is_deterministic_and_id_sensitive() {
        assert_eq!(building_hash("B0010000000AWO4JV"), building_hash("B0010000000AWO4JV"));
        assert_ne!(building_hash("B0010000000AWO4JV"), building_hash("B0010000000AWO4JW"));
    }
}
