//! SPEC_Build.md M2 "정류소 좌표 배치" + `stops.json` 출력: 대상 노선의
//! `kr_bus_routes` 항목을 `kr_bus_stops`와 매칭해(`kr_bus_stops::matching`)
//! 좌표 있는 정류소 목록을 만든다.
//!
//! **이 문서는 검수 대상이다, 확정본이 아니다.** `stop_code`-`bstopid`
//! 크로스워크가 없어 이름+노선 형상 기반 추정으로 위치를 정했으므로
//! (`kr_bus_stops::matching`의 모듈 문서 참고), 매 정류소마다 `match` 필드에
//! 방법(`method`)과 신뢰도(`confidence`)를 남긴다 -- 나중에 실제 크로스워크
//! 자료가 생기면 그 정류소의 `pos`/`match`만 갈아 끼우면 되도록.
//!
//! **대상 노선 판정과 절단 규칙**: `SPEC_Scope_v0.2.md §4.1`로 통일됐다 --
//! 노선이든 정류소든 "scope 안인가" 하나로만 묻는다. 이 모듈은 더 이상
//! 지역을 하드코딩하지 않는다; 호출부가 [`kr_scope::ScopePieceInput`]의
//! 목록(예: [`kr_scope::presets::yeongdo`])을 넘기면 [`resolve_scope_pieces`]가
//! 그걸 실제 [`kr_scope::Scope`]로 바꾸고, [`kr_scope::Scope::contains_for_route`]
//! 하나로 노선 채택·정류소 절단을 전부 판정한다. 예전의 "508(주 노선)은
//! 항상 전 구간 유지" 특례는 이제 코드 분기가 아니라 **508을 route_strip
//! scope 조각으로 넣은 프리셋의 자연스러운 결과**다 -- route_strip은 자기
//! 자신의 정류소 순서로 만든 폴리라인이라, 그 노선의 모든 정류소는 자기
//! 폴리라인의 정점이므로 거리 0으로 항상 scope 안이다.
//!
//! **`contains`가 아니라 `contains_for_route`를 쓰는 이유** (`SPEC_Scope
//! §4.1.1` "조각의 두 역할"): route_strip을 모든 노선에 똑같이 적용되는
//! 전역 판정(`contains`)에 썼더니, 508이 지나는 도심 환승 거점을 스치기만
//! 하는 무관한 노선까지 전부 채택돼 영도 프리셋 검증에서 노선 수가
//! 20 -> 51개로 늘었다 (2026-09-18 실측, `PROGRESS.md §7` item 1). route_strip
//! 조각은 **자기 노선 자신의 판정에만** 관여해야 한다 -- 지형·도로·건물
//! 생성 범위(어딘가 이 파이프라인이 `contains`를 직접 쓰게 될 부분)는
//! 여전히 route_strip을 포함한 전 조각의 합집합을 쓴다; 노선/정류소
//! 채택만 예외다.
//!
//! 정류소 판정에는 `kr_scope::STOP_SCOPE_TOLERANCE_M`만큼 경계 여유를 둔다
//! (`SPEC_Scope §4.1.2`) -- 노선 채택 판정과 절단 판정 둘 다 여기에 건다.
//! 잘라낸 정류소는 `routes[].stops`에서만 빠진다; 다른 노선(주로 508)이
//! 여전히 참조하면 `stops[]` 전역 목록에는 남는다.
//!
//! `y2`(지형고도)와 `shelter`(승차대 유무)는 이 단계에서 낼 수 없다 --
//! 전자는 `Ground`가, 후자는 M5 가로 요소 자료가 필요한데 둘 다 이 매칭
//! 단계에는 없다. `null`로 남겨 "0"을 실제 값으로 오인하지 않게 한다.

use crate::kr_bus_routes::load_route_stops;
use crate::kr_bus_stops::matching::{self, Confidence, RouteMatchResult, UnplacedReason};
use crate::kr_bus_stops::{load_bus_stops, BusStop};
use crate::kr_scope::{Scope, ScopePiece, ScopePieceInput, STOP_SCOPE_TOLERANCE_M};
use crate::projection::korea_tm::{KoreaPlanarBBox, KoreaTmProjection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Turns a preset's [`ScopePieceInput`]s into real geometry. A `RouteStrip`
/// needs a route's own matched-stop sequence to become a polyline, which
/// only exists after Pass 1 matching runs -- hence the extra `all_results`/
/// `stops` arguments this can't get from `ScopePieceInput` alone.
///
/// A route id a preset names but this run's CSV doesn't have (wrong CSV,
/// route renamed, etc.) is skipped with a warning, not treated as fatal --
/// one missing piece shouldn't abort a run when the scope's other pieces
/// (typically a rect) still define something usable.
fn resolve_scope_pieces(
    inputs: &[ScopePieceInput],
    all_results: &HashMap<&str, RouteMatchResult>,
    stops: &HashMap<u64, BusStop>,
) -> Scope {
    let mut pieces = Vec::with_capacity(inputs.len());
    for input in inputs {
        match input {
            ScopePieceInput::Rect { lat_min, lon_min, lat_max, lon_max } => {
                pieces.push(ScopePiece::Rect { lat_min: *lat_min, lon_min: *lon_min, lat_max: *lat_max, lon_max: *lon_max });
            }
            ScopePieceInput::RouteStrip { route_id, buffer_m } => {
                let Some(result) = all_results.get(route_id.as_str()) else {
                    eprintln!("Warning: KR transit: scope preset references route {route_id}, not found in this run's route CSV -- piece skipped");
                    continue;
                };
                let points_en: Vec<(f64, f64)> =
                    result.matched.iter().map(|m| KoreaTmProjection::project_raw(stops[&m.bstopid].lat, stops[&m.bstopid].lon)).collect();
                if points_en.is_empty() {
                    eprintln!("Warning: KR transit: scope preset's route {route_id} matched no stops -- route_strip piece skipped");
                    continue;
                }
                pieces.push(ScopePiece::RouteStrip { route_id: route_id.clone(), points_en, buffer_m: *buffer_m });
            }
        }
    }
    Scope::new(pieces)
}

#[derive(Serialize)]
struct StopMatchInfo {
    method: &'static str,
    confidence: &'static str,
    candidates: usize,
}

#[derive(Serialize)]
pub struct StopEntry {
    stop_id: String,
    name: String,
    pos: [i32; 2],
    y2: Option<i32>,
    routes: Vec<String>,
    shelter: Option<bool>,
    #[serde(rename = "match")]
    match_info: StopMatchInfo,
}

impl StopEntry {
    /// SPEC_Build.md M5 §3's read side: real stop coordinates -- `y2` and
    /// `shelter` stay `None` here (this module doc's own reason: neither
    /// `Ground` nor M5's own furniture-column resolution exist at M2 time),
    /// M5 computes both itself against the already-built world instead.
    pub fn pos(&self) -> (i32, i32) {
        (self.pos[0], self.pos[1])
    }
}

#[derive(Serialize)]
struct RouteEntry {
    route_id: String,
    r#type: &'static str,
    links: Vec<String>,
    stops: Vec<String>,
}

#[derive(Serialize)]
struct UnplacedEntry {
    route_id: String,
    seq: u32,
    stop_code: String,
    stop_name: String,
    reason: &'static str,
}

#[derive(Serialize)]
pub struct StopsDocument {
    stops: Vec<StopEntry>,
    routes: Vec<RouteEntry>,
    unplaced: Vec<UnplacedEntry>,
}

impl StopsDocument {
    pub fn stops(&self) -> &[StopEntry] {
        &self.stops
    }
}

/// Writes `stops.json` (SPEC_Build.md §3.3) into `dir` -- the world's parent
/// directory, the same convention `roadgraph.json`/`manifest.json` use.
pub fn write_stops_json(dir: &Path, doc: &StopsDocument) -> Result<(), String> {
    let text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("stops.json"), text).map_err(|e| e.to_string())
}

/// Per-route tallies for the console report -- SPEC_Build.md §1's accuracy
/// requirement means these counts, not just a pass/fail, are what a human
/// reviewer needs before trusting this output. `high`/`medium`/`low`/
/// `unplaced`/`abnormal_hop_count` are measured against the *full* matched
/// route (every CSV stop); `kept`/`cut` reflect the truncation rule applied
/// on top of that (508: `cut` is always 0).
pub struct RouteReport {
    pub route_no: String,
    pub total_csv_stops: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub unplaced: usize,
    pub abnormal_hop_count: usize,
    pub kept: usize,
    pub cut: usize,
}

impl RouteReport {
    pub fn flagged(&self) -> bool {
        self.unplaced > 0 || self.abnormal_hop_count > 0
    }
}

fn confidence_str(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
    }
}

fn reason_str(r: UnplacedReason) -> &'static str {
    match r {
        UnplacedReason::NoNameMatch => "no_name_match",
        UnplacedReason::AmbiguousNoRouteContext { .. } => "ambiguous_no_route_context",
    }
}

/// Loads both datasets, matches **every** route in the CSV once (needed to
/// resolve `scope_pieces`' route_strip pieces and to know which routes
/// qualify at all -- see the module doc), selects every route touching
/// `scope` (`STOP_SCOPE_TOLERANCE_M` slack, `SPEC_Scope §4.1.1`), applies the
/// truncation rule, and builds the `stops.json` document plus a per-route
/// report.
pub fn build_stops_document(
    bus_stops_dir: &Path,
    route_csv: &Path,
    planar: &KoreaPlanarBBox,
    scale: f64,
    scope_pieces: &[ScopePieceInput],
) -> Result<(StopsDocument, Vec<RouteReport>, Scope), String> {
    let stops = load_bus_stops(bus_stops_dir, planar, scale)?;
    let route_data = load_route_stops(route_csv)?;
    let name_index = matching::build_name_index(&stops);

    println!(
        "KR transit: {} stops loaded, {} routes loaded (baseline {})",
        stops.len(),
        route_data.stops_by_route.len(),
        route_data.reference_date
    );

    let mut route_names: Vec<&String> = route_data.stops_by_route.keys().collect();
    route_names.sort();

    // Pass 1: every route has to be matched before scope can even be
    // resolved -- a route_strip piece needs a route's own matched-stop
    // polyline, and the qualification test needs real coordinates.
    let mut all_results: HashMap<&str, RouteMatchResult> = HashMap::with_capacity(route_names.len());
    for &route_no in &route_names {
        let route_stops = &route_data.stops_by_route[route_no];
        all_results.insert(route_no.as_str(), matching::match_route(route_stops, &name_index, &stops));
    }

    let scope = resolve_scope_pieces(scope_pieces, &all_results, &stops);

    let mut target_routes: Vec<&str> = route_names
        .iter()
        .map(|s| s.as_str())
        .filter(|&route_no| {
            all_results[route_no].matched.iter().any(|m| {
                // A substring-fallback match is a guess (see
                // `matching::StopNameIndex::candidates_for`), not evidence a
                // route actually reaches the scope -- one bad guess must not
                // pull an unrelated, out-of-scope route into the result.
                // Route-order geometry still uses these matches (they can
                // end up in `stops.json`), just not as the signal that
                // qualifies the whole route.
                !m.via_substring && scope.contains_for_route(route_no, stops[&m.bstopid].lat, stops[&m.bstopid].lon, STOP_SCOPE_TOLERANCE_M)
            })
        })
        .collect();
    target_routes.sort();
    println!("KR transit: {} of {} routes qualify (scope match)", target_routes.len(), route_names.len());

    let mut stop_entries: HashMap<u64, (HashSet<String>, StopMatchInfo)> = HashMap::new();
    let mut route_entries = Vec::new();
    let mut unplaced_entries = Vec::new();
    let mut reports = Vec::new();

    for &route_no in &target_routes {
        let result = &all_results[route_no];
        let total_csv_stops = route_data.stops_by_route[route_no].len();

        let (mut high, mut medium, mut low) = (0usize, 0usize, 0usize);
        for m in &result.matched {
            match m.confidence {
                Confidence::High => high += 1,
                Confidence::Medium => medium += 1,
                Confidence::Low => low += 1,
            }
        }

        let mut route_stop_ids = Vec::new();
        let mut cut = 0usize;
        for m in &result.matched {
            let s = &stops[&m.bstopid];
            // No `route_no == "508"` special case: a route_strip piece
            // built from a route's own matched stops (§ module doc) keeps
            // every one of them automatically, since each is a vertex of
            // its own polyline (distance 0, always within any buffer) --
            // and `contains_for_route` only lets *this* route's own
            // route_strip count, so it can't be widened by someone else's.
            if !scope.contains_for_route(route_no, s.lat, s.lon, STOP_SCOPE_TOLERANCE_M) {
                cut += 1;
                continue;
            }
            let stop_id = m.bstopid.to_string();
            route_stop_ids.push(stop_id);
            let method = if m.via_substring {
                "substring_fallback"
            } else if m.candidate_count == 1 {
                "unambiguous"
            } else {
                "route_geometry"
            };
            stop_entries
                .entry(m.bstopid)
                .and_modify(|(routes, _)| {
                    routes.insert(route_no.to_string());
                })
                .or_insert_with(|| {
                    let mut routes = HashSet::new();
                    routes.insert(route_no.to_string());
                    (
                        routes,
                        StopMatchInfo { method, confidence: confidence_str(m.confidence), candidates: m.candidate_count },
                    )
                });
        }
        let kept = route_stop_ids.len();

        let flagged_now = !result.unplaced.is_empty() || !result.abnormal_hops.is_empty();
        if flagged_now {
            for hop in &result.abnormal_hops {
                println!(
                    "  [ABNORMAL HOP] route {route_no}: seq {} -> {} = {:.0} blocks (route median {:.0} blocks, {:.1}x)",
                    hop.from_seq,
                    hop.to_seq,
                    hop.distance_blocks,
                    hop.route_median_blocks,
                    hop.distance_blocks / hop.route_median_blocks.max(1e-6)
                );
            }
            for u in &result.unplaced {
                println!(
                    "  [UNPLACED] route {route_no}: seq {} \"{}\" ({}) -- {}",
                    u.seq, u.stop_name, u.stop_code,
                    reason_str(u.reason)
                );
            }
        }
        for u in &result.unplaced {
            unplaced_entries.push(UnplacedEntry {
                route_id: route_no.to_string(),
                seq: u.seq,
                stop_code: u.stop_code.clone(),
                stop_name: u.stop_name.clone(),
                reason: reason_str(u.reason),
            });
        }

        reports.push(RouteReport {
            route_no: route_no.to_string(),
            total_csv_stops,
            high,
            medium,
            low,
            unplaced: result.unplaced.len(),
            abnormal_hop_count: result.abnormal_hops.len(),
            kept,
            cut,
        });
        route_entries.push(RouteEntry {
            route_id: route_no.to_string(),
            r#type: "시내",
            links: Vec::new(), // 노선 폴리라인 구성은 이 함수 다음 단계
            stops: route_stop_ids,
        });
    }

    let stops_out: Vec<StopEntry> = stop_entries
        .into_iter()
        .map(|(bstopid, (routes, match_info))| {
            let s = &stops[&bstopid];
            let mut routes: Vec<String> = routes.into_iter().collect();
            routes.sort();
            StopEntry { stop_id: bstopid.to_string(), name: s.name.clone(), pos: [s.x, s.z], y2: None, routes, shelter: None, match_info }
        })
        .collect();

    Ok((StopsDocument { stops: stops_out, routes: route_entries, unplaced: unplaced_entries }, reports, scope))
}

/// `SPEC_Scope_v0.2.md §6`'s named parameters, shared across L0/L1
/// (`data_processing`'s tile filtering, `kr_roads`) since they're both
/// `Scope::contains_en` tolerances applied to the one `Scope` this module
/// resolves (§ [`build_stops_document`]). L2 has no equivalent constant
/// anymore -- a building's "near enough" test used to be a separate
/// `BUILDING_BUFFER_M` radius; now it's just `Scope::contains_en(.., 0.0)`,
/// since a route_strip piece already carries its own buffer width
/// (`SPEC_Scope §1.1`'s `buffer_m`, e.g. 508's 150m in the Yeongdo preset).
pub const TERRAIN_BUFFER_M: f64 = 1000.0;
pub const STUB_LENGTH_M: f64 = 50.0;

/// Fills in each [`RouteEntry`]'s `links` (SPEC_Build.md §3.2: "경유 링크
/// ID") by routing the road network between every consecutive pair of a
/// route's *kept* stops (§ [`build_stops_document`]'s truncation), and
/// returns every route's polyline in block coordinates -- SPEC_GenerationScope
/// §1.3's fallback ("정류소 순서를 따라 도로망 최단경로를 탐색") since this
/// pipeline has no link ID to build from directly. A hop with no path in the
/// clipped graph (still possible if the padding around `doc`'s own stop
/// extent didn't reach the connecting road) is skipped with a warning, not
/// treated as fatal -- SPEC_GenerationScope §1.3 already calls this
/// polyline "대략적 정확도로 충분" for its one purpose (buffer extent).
pub(crate) fn build_route_polylines(
    doc: &mut StopsDocument,
    moct_dir: &Path,
    planar: &KoreaPlanarBBox,
    scale: f64,
) -> Result<HashMap<String, Vec<[i32; 2]>>, String> {
    let stop_pos: HashMap<&str, [i32; 2]> = doc.stops.iter().map(|s| (s.stop_id.as_str(), s.pos)).collect();
    if stop_pos.is_empty() {
        return Ok(HashMap::new());
    }

    // Bounding EN box for every stop this graph might need to route
    // between, padded generously for detours around the coastline/hills
    // and for the buffer step right after this one.
    let block_to_en = |pos: [i32; 2]| -> (f64, f64) {
        let e = pos[0] as f64 / scale + planar.e_min();
        let n = planar.n_min() - pos[1] as f64 / scale;
        (e, n)
    };
    let (mut e_min, mut n_min) = (f64::MAX, f64::MAX);
    let (mut e_max, mut n_max) = (f64::MIN, f64::MIN);
    for &pos in stop_pos.values() {
        let (e, n) = block_to_en(pos);
        e_min = e_min.min(e);
        e_max = e_max.max(e);
        n_min = n_min.min(n);
        n_max = n_max.max(n);
    }
    const ROUTING_PADDING_M: f64 = 1500.0;
    let en_bbox = (e_min - ROUTING_PADDING_M, n_min - ROUTING_PADDING_M, e_max + ROUTING_PADDING_M, n_max + ROUTING_PADDING_M);

    let graph = crate::kr_roads::routing::RoadGraph::load(moct_dir, en_bbox)?;
    if graph.is_empty() {
        return Err(format!("KR transit: no road network nodes found in the routing bbox {en_bbox:?}"));
    }

    let mut polylines: HashMap<String, Vec<[i32; 2]>> = HashMap::new();
    for route in &mut doc.routes {
        let mut points_en: Vec<(f64, f64)> = Vec::with_capacity(route.stops.len());
        for stop_id in &route.stops {
            if let Some(&pos) = stop_pos.get(stop_id.as_str()) {
                points_en.push(block_to_en(pos));
            }
        }
        let mut polyline_blocks: Vec<[i32; 2]> = Vec::new();
        let mut link_ids: Vec<String> = Vec::new();
        for w in points_en.windows(2) {
            let (Some(from_node), Some(to_node)) = (graph.nearest_node(w[0].0, w[0].1), graph.nearest_node(w[1].0, w[1].1)) else {
                continue;
            };
            let Some((path_points, path_links)) = graph.shortest_path(from_node, to_node) else {
                eprintln!("Warning: KR transit: route {}: no road path between two consecutive stops, skipped", route.route_id);
                continue;
            };
            for (e, n) in path_points {
                let (x, z) = crate::kr_roads::en_to_block(e, n, planar, scale);
                polyline_blocks.push([x.round() as i32, z.round() as i32]);
            }
            for link_id in path_links {
                if link_ids.last() != Some(&link_id) {
                    link_ids.push(link_id);
                }
            }
        }
        route.links = link_ids;
        polylines.insert(route.route_id.clone(), polyline_blocks);
    }

    Ok(polylines)
}

/// Runs every M2 step in order: match + scope + truncate (§
/// [`build_stops_document`]), then routes the road network between
/// consecutive kept stops to fill in `routes[].links` (`SPEC_Build §3.2`)
/// and each route's polyline. The L0 terrain chunk list is no longer
/// computed here -- it used to be a buffer around these routed polylines,
/// which made L0 depend on road-network routing for no real reason
/// (`SPEC_Scope §2`'s L0 is `scope + TERRAIN_BUFFER`, and `scope` alone
/// already has everything that needs). `data_processing.rs` now derives L0
/// directly from the `Scope` this returns, via `Scope::contains_en`.
pub fn build_m2(
    bus_stops_dir: &Path,
    route_csv: &Path,
    moct_dir: &Path,
    planar: &KoreaPlanarBBox,
    scale: f64,
    scope_pieces: &[ScopePieceInput],
) -> Result<(StopsDocument, Vec<RouteReport>, Scope), String> {
    let (mut doc, reports, scope) = build_stops_document(bus_stops_dir, route_csv, planar, scale, scope_pieces)?;
    build_route_polylines(&mut doc, moct_dir, planar, scale)?;
    Ok((doc, reports, scope))
}

/// Prints the per-route report table plus a separate, short list of routes
/// that need a human look (unplaced stops or an abnormal hop) -- per the
/// instruction not to dump full per-stop detail for every clean route.
pub fn print_route_report(reports: &[RouteReport]) {
    println!(
        "{:<8} {:>5} {:>4} {:>6} {:>4} {:>4} {:>7} {:>5} {:>4}",
        "route", "csv", "hi", "med", "lo", "unpl", "abn.hop", "kept", "cut"
    );
    for r in reports {
        println!(
            "{:<8} {:>5} {:>4} {:>6} {:>4} {:>4} {:>7} {:>5} {:>4}{}",
            r.route_no,
            r.total_csv_stops,
            r.high,
            r.medium,
            r.low,
            r.unplaced,
            r.abnormal_hop_count,
            r.kept,
            r.cut,
            if r.flagged() { "  <-- review" } else { "" }
        );
    }
    let flagged: Vec<&RouteReport> = reports.iter().filter(|r| r.flagged()).collect();
    if flagged.is_empty() {
        println!("KR transit: no route has an unplaced stop or an abnormal hop.");
    } else {
        println!("KR transit: {} route(s) need review (unplaced stop(s) or abnormal hop(s)):", flagged.len());
        for r in &flagged {
            println!("  {} -- unplaced {}, abnormal hops {}", r.route_no, r.unplaced, r.abnormal_hop_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real, gitignored source data (`data/`) end to end for every
    /// route the Yeongdo preset (`kr_scope::presets::yeongdo`) scope-matches,
    /// and writes the resulting `stops.json` next to this crate for review
    /// -- and, separately, for diffing against the pre-refactor baseline
    /// (`data/stops_review_BASELINE_pre_scope_refactor.json`,
    /// PROGRESS.md §7 item 1's validation step). Not run by a plain `cargo
    /// test` (needs local-only data files); run explicitly with
    /// `cargo test --offline kr_transit -- --ignored --nocapture` to see the
    /// full console report.
    #[test]
    #[ignore]
    fn all_target_routes_end_to_end_against_real_data() {
        let bus_stops_dir = Path::new("data/부산광역시_버스 정류소 정보(SHP)_20250121");
        let route_csv = Path::new("data/부산광역시_버스노선별 승하차 정보_20230731.csv");
        let moct_dir = Path::new("data/[2026-08-12]NODELINKDATA");
        let planar = KoreaPlanarBBox::new(0.0, 0.0, 1_000_000.0, 1_000_000.0).unwrap();
        let scope_pieces = crate::kr_scope::presets::yeongdo();

        let (doc, reports, _scope) = build_m2(bus_stops_dir, route_csv, moct_dir, &planar, 1.75, &scope_pieces).unwrap();

        for r in &reports {
            assert_eq!(
                r.high + r.medium + r.low + r.unplaced,
                r.total_csv_stops,
                "every CSV row for route {} must be either placed or reported unplaced",
                r.route_no
            );
            assert_eq!(r.kept + r.cut, r.high + r.medium + r.low, "kept+cut must equal every placed stop for route {}", r.route_no);
        }
        // 508 must still keep its full mainland extent (route_strip self-qualifies,
        // SPEC_Scope §4.1.1) -- 부산역 is well outside the Yeongdo rect.
        assert!(
            doc.stops.iter().any(|s| s.name == "부산역" && s.routes.iter().any(|r| r == "508")),
            "508 must still reach 부산역 outside the Yeongdo rect"
        );
        for route in &doc.routes {
            if route.route_id == "508" {
                assert!(!route.links.is_empty(), "508 must have routed at least one road link");
            }
        }

        print_route_report(&reports);

        let json = serde_json::to_string_pretty(&doc).unwrap();
        std::fs::write("data/stops_review.json", &json).expect("write review output");
        println!("wrote data/stops_review.json ({} bytes)", json.len());
    }
}
