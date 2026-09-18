//! SPEC_Scope_v0.2.md — region-agnostic generation scope: a union of
//! pieces (rectangle / route strip; administrative-boundary polygon is
//! §8's open item, no data source yet). Every layer's "is this in scope"
//! question is meant to funnel through here rather than reimplementing its
//! own region check -- `kr_transit` is the first caller ported to this
//! (`PROGRESS.md` §7 item 1: replaces `PRIMARY_ROUTE`/`YEONGDO_LON_MIN` etc.
//! outright, not a CLI flag around them).

use crate::projection::korea_tm::KoreaTmProjection;

/// One scope piece, already resolved to real geometry (SPEC_Scope §1.1).
/// `RouteStrip`'s `points_en` are EPSG:5186 easting/northing metres, in
/// route order. Resolving a route id into this polyline needs matched-stop
/// data this module doesn't have -- see `kr_transit::resolve_scope_pieces`,
/// which turns a [`ScopePieceInput::RouteStrip`] into this.
///
/// `RouteStrip` carries its own `route_id` because it plays a narrower role
/// than `Rect` (SPEC_Scope §4.1.1: "조각의 두 역할") -- it only ever widens
/// *that* route's own scope, never another route's. Using it as a universal
/// piece was tried and measured: on the Yeongdo preset it pulled 31
/// unrelated mainland routes into scope just for sharing a few hundred
/// metres of 508's downtown corridor (PROGRESS.md §7 item 1's validation
/// log, 2026-09-18).
pub enum ScopePiece {
    Rect { lat_min: f64, lon_min: f64, lat_max: f64, lon_max: f64 },
    RouteStrip { route_id: String, points_en: Vec<(f64, f64)>, buffer_m: f64 },
}

/// SPEC_Scope §1.2: scope = the union of its pieces. A coordinate is in
/// scope if it lies in *any* piece -- overlap between pieces is harmless,
/// it just wastes a redundant check.
pub struct Scope {
    pieces: Vec<ScopePiece>,
}

impl Scope {
    pub fn new(pieces: Vec<ScopePiece>) -> Self {
        Self { pieces }
    }

    /// The *physical* generation scope (SPEC_Scope §2's L0-L3): every piece
    /// counts, `RouteStrip` included, regardless of whose route it is --
    /// terrain/roads/buildings along a route_strip's corridor genuinely
    /// need to exist for that route to be drivable. `tolerance_m` widens
    /// every piece by that many metres before testing; pass `0.0` for
    /// roads/buildings (§4.1.2) and `STOP_SCOPE_TOLERANCE_M` for stops.
    ///
    pub fn contains(&self, lat: f64, lon: f64, tolerance_m: f64) -> bool {
        self.pieces.iter().any(|p| p.contains(lat, lon, tolerance_m))
    }

    /// [`Self::contains`] for a caller already in EPSG:5186 easting/northing
    /// metres (roads, buildings, tiles) instead of lat/lon -- unprojects
    /// once and reuses `contains` so Rect/RouteStrip matching never
    /// diverges between the two coordinate systems. The extra trig is
    /// negligible at the call granularity every user of this (one test per
    /// road link, per building, per tile) actually needs.
    pub fn contains_en(&self, e: f64, n: f64, tolerance_m: f64) -> bool {
        let (lat, lon) = KoreaTmProjection::unproject_raw(e, n);
        self.contains(lat, lon, tolerance_m)
    }

    /// The scope that decides whether `route_id` itself, or one of its
    /// stops, is kept (SPEC_Scope §4.1.1): every `Rect`/`AdminPolygon`
    /// piece counts as before, but a `RouteStrip` counts **only** when it
    /// names this same `route_id` -- another route's own route_strip never
    /// widens this test. This is `kr_transit`'s route/stop qualification
    /// test; nothing else should need it.
    pub fn contains_for_route(&self, route_id: &str, lat: f64, lon: f64, tolerance_m: f64) -> bool {
        self.pieces.iter().any(|p| match p {
            ScopePiece::RouteStrip { route_id: piece_route, .. } => piece_route == route_id && p.contains(lat, lon, tolerance_m),
            _ => p.contains(lat, lon, tolerance_m),
        })
    }
}

impl ScopePiece {
    fn contains(&self, lat: f64, lon: f64, tolerance_m: f64) -> bool {
        match self {
            ScopePiece::Rect { lat_min, lon_min, lat_max, lon_max } => {
                if tolerance_m <= 0.0 {
                    (*lat_min..=*lat_max).contains(&lat) && (*lon_min..=*lon_max).contains(&lon)
                } else {
                    // A degrees-per-metre pad is an approximation, but
                    // SPEC_Scope §4.1.1's tolerance is boundary slack (tens
                    // of metres), not the generation extent itself -- the
                    // approximation error at that scale doesn't matter.
                    let lat_pad = tolerance_m / 111_320.0;
                    let lon_pad = tolerance_m / (111_320.0 * lat.to_radians().cos().max(1e-6));
                    (*lat_min - lat_pad..=*lat_max + lat_pad).contains(&lat)
                        && (*lon_min - lon_pad..=*lon_max + lon_pad).contains(&lon)
                }
            }
            ScopePiece::RouteStrip { points_en, buffer_m, .. } => {
                let (pe, pn) = KoreaTmProjection::project_raw(lat, lon);
                let effective = buffer_m + tolerance_m;
                if points_en.len() < 2 {
                    return points_en.iter().any(|&(e, n)| ((pe - e).powi(2) + (pn - n).powi(2)).sqrt() <= effective);
                }
                points_en.windows(2).any(|w| point_segment_distance_en(w[0], w[1], (pe, pn)) <= effective)
            }
        }
    }
}

fn point_segment_distance_en(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    let (dx, dn) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dn * dn;
    if len2 < 1e-9 {
        return ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt();
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dn) / len2).clamp(0.0, 1.0);
    let (cx, cn) = (a.0 + t * dx, a.1 + t * dn);
    ((p.0 - cx).powi(2) + (p.1 - cn).powi(2)).sqrt()
}

/// SPEC_Scope §4.1.1: boundary slack applied only to stop-inclusion tests,
/// never to roads/buildings -- a missed stop is costlier than an extra one
/// (stop position is the one thing that crosses to McRIG intact, §7.2).
pub const STOP_SCOPE_TOLERANCE_M: f64 = 50.0;

/// A scope piece as a caller (a region preset) specifies it, before a
/// route id is resolved into real polyline geometry.
pub enum ScopePieceInput {
    Rect { lat_min: f64, lon_min: f64, lat_max: f64, lon_max: f64 },
    RouteStrip { route_id: String, buffer_m: f64 },
}

/// SPEC_Scope §7 "영도 프리셋 예시" as data, not code branches -- swap this
/// for a different region's pieces (or an actual preset-file loader, once
/// one exists, SPEC_Scope §5) without touching `kr_transit`'s matching or
/// filtering logic at all.
pub mod presets {
    use super::ScopePieceInput;

    /// §7.1: 영도구 전역을 감싸는 최소 사각형(행정구역 폴리곤 데이터가 아직
    /// 없어 사각형으로 대체 -- SPEC_Scope §8) ∪ 508 노선 띠(전 구간 유지용,
    /// 폭 150m). 이 둘의 합집합이 예전 `is_in_yeongdo_range` + `PRIMARY_ROUTE`
    /// 특례가 하던 일을 대신한다.
    pub fn yeongdo() -> Vec<ScopePieceInput> {
        vec![
            ScopePieceInput::Rect { lat_min: 35.060, lon_min: 129.037, lat_max: 35.100, lon_max: 129.090 },
            ScopePieceInput::RouteStrip { route_id: "508".to_string(), buffer_m: 150.0 },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yeongdo_rect() -> ScopePiece {
        ScopePiece::Rect { lat_min: 35.060, lon_min: 129.037, lat_max: 35.100, lon_max: 129.090 }
    }

    /// Ported from `kr_transit`'s old `is_in_yeongdo_range` tests -- same
    /// confirmed-mainland points, same expectation, now phrased as a scope
    /// query instead of a bespoke range function.
    #[test]
    fn rect_excludes_the_confirmed_mainland_cluster() {
        let scope = Scope::new(vec![yeongdo_rect()]);
        assert!(!scope.contains(35.098124820320002, 129.029504173349011, 0.0));
        assert!(!scope.contains(35.098276349999999, 129.029738199999997, 0.0));
        assert!(!scope.contains(35.095626666699999, 129.024266666699987, 0.0));
        assert!(!scope.contains(35.101258333300002, 129.025168333300002, 0.0));
    }

    #[test]
    fn rect_includes_confirmed_yeongdo_landmarks() {
        let scope = Scope::new(vec![yeongdo_rect()]);
        assert!(scope.contains(35.077, 129.0455, 0.0));
        assert!(scope.contains(35.064896666700001, 129.081115000000011, 0.0));
        assert!(scope.contains(35.076531795698003, 129.087808534199013, 0.0));
    }

    #[test]
    fn rect_tolerance_pulls_in_a_nearby_outside_point_but_not_a_far_one() {
        // Just outside the mainland-cluster test's northern boundary point
        // (lat_max = 35.100): a hair over the line.
        let scope = Scope::new(vec![yeongdo_rect()]);
        let just_outside = (35.1003, 129.06); // ~33m north of lat_max
        assert!(!scope.contains(just_outside.0, just_outside.1, 0.0));
        assert!(scope.contains(just_outside.0, just_outside.1, STOP_SCOPE_TOLERANCE_M));
        let far_outside = (35.150, 129.200);
        assert!(!scope.contains(far_outside.0, far_outside.1, STOP_SCOPE_TOLERANCE_M));
    }

    #[test]
    fn contains_en_round_trips_through_projection_and_agrees_with_contains() {
        let scope = Scope::new(vec![yeongdo_rect()]);
        let (lat, lon) = (35.077, 129.0455); // inside, per rect_includes... above
        let (e, n) = KoreaTmProjection::project_raw(lat, lon);
        assert!(scope.contains_en(e, n, 0.0));
        let (lat, lon) = (35.150, 129.200); // far outside
        let (e, n) = KoreaTmProjection::project_raw(lat, lon);
        assert!(!scope.contains_en(e, n, 0.0));
    }

    #[test]
    fn route_strip_includes_points_near_its_polyline_and_excludes_far_ones() {
        let a = KoreaTmProjection::project_raw(35.070, 129.045);
        let b = KoreaTmProjection::project_raw(35.075, 129.050);
        let scope = Scope::new(vec![ScopePiece::RouteStrip { route_id: "508".to_string(), points_en: vec![a, b], buffer_m: 150.0 }]);
        // The polyline's own endpoints must be in scope (distance 0) --
        // this is what lets a route_strip built from a route's own matched
        // stops keep every one of them, reproducing the old PRIMARY_ROUTE
        // "keep all" behaviour without a special case.
        assert!(scope.contains(35.070, 129.045, 0.0));
        assert!(scope.contains(35.075, 129.050, 0.0));
        // Far from the segment, well past the buffer.
        assert!(!scope.contains(35.150, 129.200, 0.0));
    }

    /// SPEC_Scope §4.1.1 "조각의 두 역할": a route_strip only widens *its
    /// own* route's qualification test, never another route's -- this is
    /// the fix for the 20->51 route blow-up the first cut of this module
    /// measured (PROGRESS.md §7 item 1's validation log).
    #[test]
    fn route_strip_only_self_qualifies_not_other_routes() {
        let corridor = ScopePiece::RouteStrip {
            route_id: "508".to_string(),
            points_en: vec![KoreaTmProjection::project_raw(35.070, 129.045), KoreaTmProjection::project_raw(35.100, 129.030)],
            buffer_m: 150.0,
        };
        let scope = Scope::new(vec![yeongdo_rect(), corridor]);
        // A point on 508's corridor but well outside the Yeongdo rect.
        let (lat, lon) = (35.100, 129.030);
        assert!(scope.contains_for_route("508", lat, lon, 0.0), "508 must see its own corridor");
        assert!(!scope.contains_for_route("2", lat, lon, 0.0), "an unrelated route must not be pulled in by 508's corridor");
        // The physical generation scope, by contrast, does include it --
        // buildings/terrain along 508's own route must still exist.
        assert!(scope.contains(lat, lon, 0.0));
    }
}
