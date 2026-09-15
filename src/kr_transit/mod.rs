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
//! **대상 노선 판정**: SPEC_GenerationScope.md §1.1의 주 노선(508)은 항상
//! 포함한다. 그 외 노선은 매칭된 정류소 중 하나라도 [`is_in_yeongdo_range`]
//! 범위에 들면 포함한다 -- 구/동 필드가 어디에도 없어 좌표 범위가 유일하게
//! 가진 판정 신호다 (해당 함수 문서 참고).
//!
//! **절단 규칙** (SPEC_GenerationScope.md §1.1, 2026-09-13 개정): 508은
//! 매칭된 정류소를 전부 유지한다. 그 외 대상 노선은 [`is_in_yeongdo_range`]
//! 범위 밖으로 나가는 정류소를 잘라낸다 -- 다리를 건넌 이후 구간이라는
//! 뜻이다. 잘라낸 정류소는 `routes[].stops`에서만 빠진다; 다른 노선(주로
//! 508)이 여전히 참조하면 `stops[]` 전역 목록에는 남는다.
//!
//! `y2`(지형고도)와 `shelter`(승차대 유무)는 이 단계에서 낼 수 없다 --
//! 전자는 `Ground`가, 후자는 M5 가로 요소 자료가 필요한데 둘 다 이 매칭
//! 단계에는 없다. `null`로 남겨 "0"을 실제 값으로 오인하지 않게 한다.

use crate::kr_bus_routes::load_route_stops;
use crate::kr_bus_stops::load_bus_stops;
use crate::kr_bus_stops::matching::{self, Confidence, RouteMatchResult, UnplacedReason};
use crate::projection::korea_tm::KoreaPlanarBBox;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// SPEC_GenerationScope.md §1.1's 주 노선 -- always included, never truncated.
pub const PRIMARY_ROUTE: &str = "508";

const YEONGDO_LON_MIN: f64 = 129.037;
const YEONGDO_LON_MAX: f64 = 129.090;
const YEONGDO_LAT_MIN: f64 = 35.060;
const YEONGDO_LAT_MAX: f64 = 35.100;

/// A coordinate rectangle standing in for "영도구" -- there is no
/// administrative-boundary polygon in this pipeline, and the bus stop SHP
/// carries no 구/동 field either (confirmed by inspecting its actual
/// fields), so a coordinate range is the only signal available.
///
/// Derived from the real data, not guessed: `arsno`'s first two digits are
/// Busan's bus-stop ARS region code, and `04` was cross-validated earlier
/// against unambiguous Yeongdo landmarks (대교동, 남항동, 동삼동, 고신대학,
/// HJ중공업, 국립해양박물관, 한국해양대 ...). Every `arsno`-`04` stop falls
/// in lon [129.037257, 129.081256], lat [35.063414, 35.099485]; this range
/// pads that envelope slightly (to cover a few real Yeongdo stops just
/// outside it, e.g. 한국해양대 해사대학관 at lon 129.0887) while staying
/// clear of the nearest confirmed mainland cluster (자갈치역·남포동·
/// 충무동교차로·부평시장, all lon <= 129.0298).
///
/// **Known limit**: the two bridgeheads (영도대교/부산대교 남포동·중앙동
/// 쪽 접속부) sit only ~100-300m from this range's own edge, closer than
/// the padding above -- a stop right at a bridge approach can land on
/// either side of the line. This is exactly the ambiguity
/// SPEC_GenerationScope.md §1.1 now discloses for the truncation rule: at
/// most one stop of error at the cut point, not a systematic
/// misclassification of interior stops.
pub fn is_in_yeongdo_range(lat: f64, lon: f64) -> bool {
    (YEONGDO_LAT_MIN..=YEONGDO_LAT_MAX).contains(&lat) && (YEONGDO_LON_MIN..=YEONGDO_LON_MAX).contains(&lon)
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
/// decide which ones qualify at all -- see the module doc), selects 508 plus
/// every route touching [`is_in_yeongdo_range`], applies the truncation rule
/// to the non-508 selections, and builds the `stops.json` document plus a
/// per-route report.
pub fn build_stops_document(
    bus_stops_dir: &Path,
    route_csv: &Path,
    planar: &KoreaPlanarBBox,
    scale: f64,
) -> Result<(StopsDocument, Vec<RouteReport>), String> {
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

    // Pass 1: every route has to be matched before we know which ones even
    // touch Yeongdo -- the qualification test needs real coordinates.
    let mut all_results: HashMap<&str, RouteMatchResult> = HashMap::with_capacity(route_names.len());
    for &route_no in &route_names {
        let route_stops = &route_data.stops_by_route[route_no];
        all_results.insert(route_no.as_str(), matching::match_route(route_stops, &name_index, &stops));
    }

    let mut target_routes: Vec<&str> = route_names
        .iter()
        .map(|s| s.as_str())
        .filter(|&route_no| {
            route_no == PRIMARY_ROUTE
                || all_results[route_no].matched.iter().any(|m| {
                    // A substring-fallback match is a guess (see
                    // `matching::StopNameIndex::candidates_for`), not
                    // evidence a route actually reaches Yeongdo -- one bad
                    // guess must not pull an unrelated, otherwise-mainland
                    // route into scope. Route-order geometry still uses
                    // these matches (they can end up in `stops.json`), just
                    // not as the signal that qualifies the whole route.
                    !m.via_substring && is_in_yeongdo_range(stops[&m.bstopid].lat, stops[&m.bstopid].lon)
                })
        })
        .collect();
    target_routes.sort();
    println!(
        "KR transit: {} of {} routes qualify (508 + coordinate-based Yeongdo match)",
        target_routes.len(),
        route_names.len()
    );

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

        let keep_all = route_no == PRIMARY_ROUTE;
        let mut route_stop_ids = Vec::new();
        let mut cut = 0usize;
        for m in &result.matched {
            let s = &stops[&m.bstopid];
            if !keep_all && !is_in_yeongdo_range(s.lat, s.lon) {
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

    Ok((StopsDocument { stops: stops_out, routes: route_entries, unplaced: unplaced_entries }, reports))
}

/// SPEC_GenerationScope.md §6's named parameters. `STUB_LENGTH_M`
/// (branch-road stub length) belongs to L1 road generation, not this
/// module's buffer/chunk-list computation -- named here anyway so all three
/// live next to their shared source, and picked up once L1 is wired in.
pub const TERRAIN_BUFFER_M: f64 = 1000.0;
pub const BUILDING_BUFFER_M: f64 = 150.0;
#[allow(dead_code)]
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

fn point_segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dz * dz;
    if len2 < 1e-9 {
        return ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt();
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dz) / len2).clamp(0.0, 1.0);
    let (cx, cz) = (a.0 + t * dx, a.1 + t * dz);
    ((p.0 - cx).powi(2) + (p.1 - cz).powi(2)).sqrt()
}

/// SPEC_GenerationScope.md §0/§2: the chunks (16-block Minecraft columns)
/// within `buffer_blocks` of any point on any route's polyline -- the L0
/// terrain footprint, which per §4 "L0 버퍼가 전체 예산을 좌우한다" is what
/// actually bounds the chunk budget (L2's tighter building buffer is a mask
/// *within* this set, not a separate inclusion boundary; L3's whole-rectangle
/// high-rise scan is deferred to M4, when building data exists to scan at
/// all -- this function doesn't claim to cover it).
///
/// Walked per polyline *segment*, not per point: a per-point disc of this
/// radius would repeat the same chunk cells from every one of a polyline's
/// many closely-spaced points, while a segment's own bounding rectangle
/// (padded by the buffer) is barely larger than the segment itself.
pub fn buffer_chunks(polylines: &HashMap<String, Vec<[i32; 2]>>, buffer_blocks: i32) -> std::collections::HashSet<(i32, i32)> {
    const CHUNK: i32 = 16;
    let mut chunks = std::collections::HashSet::new();
    for points in polylines.values() {
        for w in points.windows(2) {
            let a = (w[0][0] as f64, w[0][1] as f64);
            let b = (w[1][0] as f64, w[1][1] as f64);
            let min_x = a.0.min(b.0) as i32 - buffer_blocks;
            let max_x = a.0.max(b.0) as i32 + buffer_blocks;
            let min_z = a.1.min(b.1) as i32 - buffer_blocks;
            let max_z = a.1.max(b.1) as i32 + buffer_blocks;
            let (cx0, cx1) = (min_x.div_euclid(CHUNK), max_x.div_euclid(CHUNK));
            let (cz0, cz1) = (min_z.div_euclid(CHUNK), max_z.div_euclid(CHUNK));
            for cx in cx0..=cx1 {
                for cz in cz0..=cz1 {
                    if chunks.contains(&(cx, cz)) {
                        continue;
                    }
                    let center = ((cx * CHUNK + CHUNK / 2) as f64, (cz * CHUNK + CHUNK / 2) as f64);
                    if point_segment_distance(center, a, b) <= buffer_blocks as f64 {
                        chunks.insert((cx, cz));
                    }
                }
            }
        }
    }
    chunks
}

/// Runs every M2 step in order: match + scope + truncate (§
/// [`build_stops_document`]), route the road network between consecutive
/// kept stops to fill in `routes[].links` and get each route's polyline,
/// then buffer those polylines into the L0 terrain chunk list.
pub fn build_m2(
    bus_stops_dir: &Path,
    route_csv: &Path,
    moct_dir: &Path,
    planar: &KoreaPlanarBBox,
    scale: f64,
) -> Result<(StopsDocument, Vec<RouteReport>, std::collections::HashSet<(i32, i32)>), String> {
    let (mut doc, reports) = build_stops_document(bus_stops_dir, route_csv, planar, scale)?;
    let polylines = build_route_polylines(&mut doc, moct_dir, planar, scale)?;
    let buffer_blocks = (TERRAIN_BUFFER_M * scale).round() as i32;
    let chunks = buffer_chunks(&polylines, buffer_blocks);
    println!(
        "KR transit: L0 terrain buffer ({TERRAIN_BUFFER_M:.0}m -> {buffer_blocks} blocks) covers {} chunks ({} regions)",
        chunks.len(),
        chunks.iter().map(|&(cx, cz)| (cx.div_euclid(32), cz.div_euclid(32))).collect::<std::collections::HashSet<_>>().len()
    );
    Ok((doc, reports, chunks))
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

    #[test]
    fn yeongdo_range_excludes_the_confirmed_mainland_cluster() {
        // 자갈치역.비프광장, 남포동, 충무동교차로, 부평시장 -- all confirmed
        // mainland during the loading-session investigation.
        assert!(!is_in_yeongdo_range(35.098124820320002, 129.029504173349011));
        assert!(!is_in_yeongdo_range(35.098276349999999, 129.029738199999997));
        assert!(!is_in_yeongdo_range(35.095626666699999, 129.024266666699987));
        assert!(!is_in_yeongdo_range(35.101258333300002, 129.025168333300002));
    }

    #[test]
    fn yeongdo_range_includes_confirmed_yeongdo_landmarks() {
        // 대교동 (bridge connection point), 태종대초등학교 (south tip),
        // 해양대구본관 (east end, Dongsam-dong causeway campus).
        assert!(is_in_yeongdo_range(35.077, 129.0455)); // 대교동 vicinity, approximate
        assert!(is_in_yeongdo_range(35.064896666700001, 129.081115000000011));
        assert!(is_in_yeongdo_range(35.076531795698003, 129.087808534199013));
    }

    /// Runs the real, gitignored source data (`data/`) end to end for every
    /// route the coordinate-based Yeongdo test selects, and writes the
    /// resulting `stops.json` next to this crate for review. Not run by a
    /// plain `cargo test` (needs local-only data files); run explicitly with
    /// `cargo test --offline kr_transit -- --ignored --nocapture` to see the
    /// full console report.
    #[test]
    #[ignore]
    fn all_target_routes_end_to_end_against_real_data() {
        let bus_stops_dir = Path::new("data/부산광역시_버스 정류소 정보(SHP)_20250121");
        let route_csv = Path::new("data/부산광역시_버스노선별 승하차 정보_20230731.csv");
        let moct_dir = Path::new("data/[2026-08-12]NODELINKDATA");
        let planar = KoreaPlanarBBox::new(0.0, 0.0, 1_000_000.0, 1_000_000.0).unwrap();

        let (doc, reports, chunks) = build_m2(bus_stops_dir, route_csv, moct_dir, &planar, 1.75).unwrap();

        for r in &reports {
            assert_eq!(
                r.high + r.medium + r.low + r.unplaced,
                r.total_csv_stops,
                "every CSV row for route {} must be either placed or reported unplaced",
                r.route_no
            );
            assert_eq!(r.kept + r.cut, r.high + r.medium + r.low, "kept+cut must equal every placed stop for route {}", r.route_no);
        }
        assert!(!chunks.is_empty(), "the L0 buffer must cover at least one chunk");
        for route in &doc.routes {
            if route.route_id == PRIMARY_ROUTE {
                assert!(!route.links.is_empty(), "508 must have routed at least one road link");
            }
        }

        print_route_report(&reports);

        let json = serde_json::to_string_pretty(&doc).unwrap();
        std::fs::write("data/stops_review.json", &json).expect("write review output");
        println!("wrote data/stops_review.json ({} bytes)", json.len());
    }
}
