//! Route-stop matching: connects `kr_bus_routes`' CSV rows (name + sequence
//! only) to [`super::BusStop`] (has coordinates). `stop_code` cannot be
//! joined to `bstopid` directly -- confirmed by cross-checking the real
//! files (0/4138 direct match; see `kr_bus_routes`'s module doc for the
//! full finding) -- so this matches by name, and where a name is shared by
//! several stops (chiefly opposite-direction stop pairs, common in this
//! data), disambiguates by solving the **whole route at once**: pick the
//! one candidate combination, across every ambiguous position, that makes
//! the resulting path through all of a route's stops as geometrically
//! smooth as possible.
//!
//! "Smooth" is a per-hop penalty of (a) distance to the next stop and
//! (b) how sharply the direction of travel turns, minimised over the whole
//! route as a shortest path through a small trellis (one column of
//! candidates per stop position, second-order so the turn angle -- which
//! needs three consecutive points -- has a well-defined cost). **A route
//! runs out to a terminus and back**, so a turn of close to 180 degrees is
//! the expected, unpenalised case, same as continuing straight; the penalty
//! peaks at a 90-degree turn, which is what a wrong (same-named,
//! wrong-direction) candidate actually looks like: a real sideways jump
//! rather than either progress or a legitimate reversal.
//!
//! **This produces an estimate, not a fact.** SPEC_Build.md §1 calls stop-
//! position accuracy "the one non-negotiable item", so every resolved stop
//! carries a High/Medium/Low confidence tier (below) and every unplaced one
//! is reported by name -- both meant to be replaced outright once a real
//! `stop_code`-to-`bstopid` crosswalk exists, not trusted as final.

use super::BusStop;
use crate::kr_bus_routes::RouteStop;
use std::collections::HashMap;

/// Blocks per degree of turn penalty -- see the module doc's "smooth path"
/// framing. A disclosed, tunable heuristic: not derived from the data, just
/// picked to make a 90-degree zigzag (worst case) cost roughly as much as a
/// typical real inter-stop hop, so it can outweigh a shorter-but-wrong
/// candidate without swamping genuine distance differences.
const ANGLE_PENALTY_PER_DEGREE_BLOCKS: f64 = 5.0;

/// A resolved-vs-runner-up cost ratio below this makes a disambiguated
/// stop's confidence `Low` instead of `Medium` -- the DP still had to pick
/// something, but the alternative was almost as good a fit.
const CONFIDENT_MARGIN_RATIO: f64 = 2.0;

/// A same-named alternative within this many blocks of the chosen candidate
/// is treated as that stop's opposite-direction twin, not a genuinely
/// different location -- roughly 85m at this project's 1.75 blocks/metre
/// scale, generous for a curb-to-curb or intersection-corner separation. A
/// disclosed, tunable heuristic, not derived from the data.
const CLOSE_CANDIDATE_THRESHOLD_BLOCKS: f64 = 150.0;

/// A hop flagged as abnormal if it exceeds this multiple of the route's own
/// median hop distance -- relative, not an absolute metre figure, since
/// stop spacing itself varies by road type within one route.
const ABNORMAL_HOP_MULTIPLE: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Only one `BusStop` carries this name -- no choice was made.
    High,
    /// Several stops share this name, but either the nearest alternative is
    /// close enough (opposite-direction twin) that a wrong pick would barely
    /// move the stop, or it's far away and the route-wide solve picked this
    /// one with a clear cost margin over it.
    Medium,
    /// Several stops share this name, the nearest alternative is far enough
    /// away to matter, and the margin over it was a close call -- treat as
    /// unverified.
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnplacedReason {
    /// No `BusStop` anywhere carries this name.
    NoNameMatch,
    /// Several stops share this name, but the route had fewer than two
    /// placeable positions total, so there was no path geometry to
    /// disambiguate against.
    AmbiguousNoRouteContext { candidates: usize },
}

pub struct MatchedStop {
    pub seq: u32,
    pub stop_code: String,
    pub stop_name: String,
    pub bstopid: u64,
    pub confidence: Confidence,
    pub candidate_count: usize,
    /// True when no exact, punctuation/whitespace-normalized, or token-set
    /// match existed and this candidate list came from the substring
    /// fallback -- see [`StopNameIndex::candidates_for`]. Always pairs with
    /// `Confidence::Low`, pinned regardless of the route-smoothness result.
    pub via_substring: bool,
}

pub struct UnplacedStop {
    pub seq: u32,
    pub stop_code: String,
    pub stop_name: String,
    pub reason: UnplacedReason,
}

/// One adjacent pair (in the resolved path, not necessarily adjacent `seq`
/// numbers if a stop in between was unplaced) whose distance is an outlier
/// for this route -- SPEC_Build.md §1's accuracy requirement means this is
/// where a wrong disambiguation is most likely to show up.
pub struct AbnormalHop {
    pub from_seq: u32,
    pub to_seq: u32,
    pub distance_blocks: f64,
    pub route_median_blocks: f64,
}

pub struct RouteMatchResult {
    /// In route order (by `seq`), placed positions only.
    pub matched: Vec<MatchedStop>,
    pub unplaced: Vec<UnplacedStop>,
    pub abnormal_hops: Vec<AbnormalHop>,
}

/// Characters treated as separators/noise when comparing stop names loosely
/// -- the punctuation this data actually uses to glue two landmark names
/// together (`.`, `·`) or bracket a qualifier (`()`), plus whitespace.
const NAME_SEPARATORS: [char; 9] = ['.', '·', ',', '(', ')', '/', '-', '_', ' '];

/// Strips [`NAME_SEPARATORS`] entirely -- "구두점·공백 제거 후 비교" tier.
fn normalize_name(name: &str) -> String {
    name.chars().filter(|c| !NAME_SEPARATORS.contains(c)).collect()
}

/// Splits on [`NAME_SEPARATORS`], drops empty pieces, sorts -- "구분자로
/// 나눈 조각들의 집합 비교 (어순 무시)" tier. Returned as a joined string
/// (not a `Vec`/`HashSet`) so it can be a plain `HashMap` key.
fn token_set_key(name: &str) -> String {
    let mut tokens: Vec<&str> = name.split(NAME_SEPARATORS).filter(|s| !s.is_empty()).collect();
    tokens.sort_unstable();
    tokens.join("\u{1}")
}

/// Every loaded stop, indexed four ways for [`candidates_for`]'s cascade:
/// exact name, punctuation/whitespace-normalized name, order-independent
/// token set, and (fallback only) a flat list for substring scanning. Built
/// once per run and shared across every route's matching pass -- see
/// [`build_name_index`].
pub struct StopNameIndex {
    exact: HashMap<String, Vec<u64>>,
    normalized: HashMap<String, Vec<u64>>,
    token_set: HashMap<String, Vec<u64>>,
    all_normalized: Vec<(u64, String)>,
}

impl StopNameIndex {
    /// Candidate lookup cascade for one route-stop name -- widens tier by
    /// tier only when the previous tier found nothing at all:
    /// 1. exact string match;
    /// 2. match after stripping [`NAME_SEPARATORS`];
    /// 3. match on the same set of `.`/`·`/whitespace-delimited tokens,
    ///    order ignored (catches "A.B" vs "B.A" word-order swaps);
    /// 4. substring match on the normalized forms, in either direction.
    ///
    /// Only tier 4's result is flagged (`via_substring = true`) -- tiers 1-3
    /// are still exact identity under a normalization, not a guess, so they
    /// flow into the same route-smoothness selection as an untransformed
    /// exact match with no special handling. Tier 4 requires at least 2
    /// normalized characters to guard against a short/empty name matching
    /// almost everything.
    pub fn candidates_for(&self, name: &str) -> (Vec<u64>, bool) {
        if let Some(c) = self.exact.get(name) {
            return (c.clone(), false);
        }
        let norm = normalize_name(name);
        if let Some(c) = self.normalized.get(&norm) {
            return (c.clone(), false);
        }
        if let Some(c) = self.token_set.get(&token_set_key(name)) {
            return (c.clone(), false);
        }
        if norm.chars().count() < 2 {
            return (Vec::new(), false);
        }
        let mut found: Vec<u64> = self
            .all_normalized
            .iter()
            .filter(|(_, sn)| sn.contains(&norm) || norm.contains(sn.as_str()))
            .map(|&(id, _)| id)
            .collect();
        found.sort_unstable();
        found.dedup();
        let via_substring = !found.is_empty();
        (found, via_substring)
    }
}

/// Groups every loaded stop by its own name (and normalized/token-set forms)
/// -- the lookup table every route's matching pass shares, so it should be
/// built once per run, not once per route.
pub fn build_name_index(stops: &HashMap<u64, BusStop>) -> StopNameIndex {
    let mut exact: HashMap<String, Vec<u64>> = HashMap::new();
    let mut normalized: HashMap<String, Vec<u64>> = HashMap::new();
    let mut token_set: HashMap<String, Vec<u64>> = HashMap::new();
    let mut all_normalized: Vec<(u64, String)> = Vec::with_capacity(stops.len());
    for stop in stops.values() {
        exact.entry(stop.name.clone()).or_default().push(stop.bstopid);
        let norm = normalize_name(&stop.name);
        normalized.entry(norm.clone()).or_default().push(stop.bstopid);
        token_set.entry(token_set_key(&stop.name)).or_default().push(stop.bstopid);
        all_normalized.push((stop.bstopid, norm));
    }
    StopNameIndex { exact, normalized, token_set, all_normalized }
}

fn pos(stops: &HashMap<u64, BusStop>, id: u64) -> (f64, f64) {
    let s = &stops[&id];
    (s.x as f64, s.z as f64)
}

fn dist(stops: &HashMap<u64, BusStop>, a: u64, b: u64) -> f64 {
    let (ax, az) = pos(stops, a);
    let (bx, bz) = pos(stops, b);
    ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt()
}

/// `min(theta, 180 - theta)`, `theta` the angle in degrees between the
/// `a->b` and `b->c` direction vectors: 0 both for continuing straight and
/// for a full reversal, peaking at 90 for a sideways turn. `None` if either
/// leg has zero length (coincident points -- no direction to compare).
fn turn_penalty_degrees(stops: &HashMap<u64, BusStop>, a: u64, b: u64, c: u64) -> f64 {
    let (ax, az) = pos(stops, a);
    let (bx, bz) = pos(stops, b);
    let (cx, cz) = pos(stops, c);
    let v1 = (bx - ax, bz - az);
    let v2 = (cx - bx, cz - bz);
    let m1 = (v1.0 * v1.0 + v1.1 * v1.1).sqrt();
    let m2 = (v2.0 * v2.0 + v2.1 * v2.1).sqrt();
    if m1 < 1e-6 || m2 < 1e-6 {
        return 0.0;
    }
    let cos_theta = ((v1.0 * v2.0 + v1.1 * v2.1) / (m1 * m2)).clamp(-1.0, 1.0);
    let theta = cos_theta.acos().to_degrees();
    theta.min(180.0 - theta)
}

fn edge_cost(stops: &HashMap<u64, BusStop>, prev_prev: Option<u64>, prev: u64, cur: u64) -> f64 {
    let d = dist(stops, prev, cur);
    let angle = prev_prev.map_or(0.0, |pp| turn_penalty_degrees(stops, pp, prev, cur));
    d + angle * ANGLE_PENALTY_PER_DEGREE_BLOCKS
}

/// Solves one route's whole stop sequence at once: a second-order shortest
/// path through the trellis of name candidates (state = chosen candidates
/// for the current and previous position, since the turn-angle cost needs
/// three consecutive points). Positions with zero candidates are excluded
/// from the chain entirely (reported as `unplaced`, not left as gaps that
/// would corrupt the geometry the remaining positions are scored against).
pub fn match_route(
    route_stops: &[RouteStop],
    name_index: &StopNameIndex,
    stops: &HashMap<u64, BusStop>,
) -> RouteMatchResult {
    let mut placeable: Vec<(&RouteStop, Vec<u64>, bool)> = Vec::new();
    let mut unplaced: Vec<UnplacedStop> = Vec::new();
    for rs in route_stops {
        let (candidates, via_substring) = name_index.candidates_for(&rs.stop_name);
        if candidates.is_empty() {
            unplaced.push(UnplacedStop {
                seq: rs.seq,
                stop_code: rs.stop_code.clone(),
                stop_name: rs.stop_name.clone(),
                reason: UnplacedReason::NoNameMatch,
            });
        } else {
            placeable.push((rs, candidates, via_substring));
        }
    }

    let m = placeable.len();
    if m < 2 {
        // No route geometry to disambiguate against at all: place only the
        // unambiguous ones, push any ambiguous one to `unplaced`.
        let mut matched = Vec::new();
        for (rs, candidates, via_substring) in &placeable {
            if candidates.len() == 1 {
                matched.push(MatchedStop {
                    seq: rs.seq,
                    stop_code: rs.stop_code.clone(),
                    stop_name: rs.stop_name.clone(),
                    bstopid: candidates[0],
                    confidence: if *via_substring { Confidence::Low } else { Confidence::High },
                    candidate_count: 1,
                    via_substring: *via_substring,
                });
            } else {
                unplaced.push(UnplacedStop {
                    seq: rs.seq,
                    stop_code: rs.stop_code.clone(),
                    stop_name: rs.stop_name.clone(),
                    reason: UnplacedReason::AmbiguousNoRouteContext { candidates: candidates.len() },
                });
            }
        }
        return RouteMatchResult { matched, unplaced, abnormal_hops: Vec::new() };
    }

    // dp[i]: (cur, prev) -> (cost, prev_prev used to reach it), for i >= 1.
    // i == 0 has no "prev" yet, so it is kept separately as a plain cost map.
    let mut dp0: HashMap<u64, f64> = HashMap::new();
    for &c in &placeable[0].1 {
        dp0.insert(c, 0.0);
    }
    let mut dp: Vec<HashMap<(u64, u64), (f64, Option<u64>)>> = Vec::with_capacity(m);
    {
        let mut dp1 = HashMap::new();
        for &cur in &placeable[1].1 {
            for (&prev, &prev_cost) in &dp0 {
                let cost = prev_cost + edge_cost(stops, None, prev, cur);
                dp1.entry((cur, prev))
                    .and_modify(|(best, _)| {
                        if cost < *best {
                            *best = cost;
                        }
                    })
                    .or_insert((cost, None));
            }
        }
        dp.push(dp1);
    }
    for i in 2..m {
        let prev_layer = &dp[i - 2]; // dp index (i-2) corresponds to placeable position (i-1)
        let mut layer = HashMap::new();
        for &cur in &placeable[i].1 {
            for &prev in &placeable[i - 1].1 {
                let mut best: Option<(f64, Option<u64>)> = None;
                for (&(p, pp), &(pcost, _)) in prev_layer.iter() {
                    if p != prev {
                        continue;
                    }
                    let cost = pcost + edge_cost(stops, Some(pp), prev, cur);
                    if best.is_none_or(|(b, _)| cost < b) {
                        best = Some((cost, Some(pp)));
                    }
                }
                if let Some((cost, pp)) = best {
                    layer.insert((cur, prev), (cost, pp));
                }
            }
        }
        dp.push(layer);
    }

    let last = dp.last().unwrap();
    let (&(last_cur, last_prev), _) = last
        .iter()
        .min_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
        .expect("placeable.len() >= 2 guarantees at least one dp1 entry");

    let mut chosen = vec![0u64; m];
    chosen[m - 1] = last_cur;
    chosen[m - 2] = last_prev;
    for i in (0..m - 2).rev() {
        // `dp[k]` holds placeable position `k+1`'s layer (position 0 isn't
        // pushed into `dp` -- it's `dp0` above). We want position `i+2`'s
        // layer, keyed by (candidate@i+2, candidate@i+1), to recover the
        // `prev_prev` (candidate@i) it was built from -- so index `dp[i+1]`.
        let (_, prev_prev) = dp[i + 1][&(chosen[i + 2], chosen[i + 1])];
        chosen[i] = prev_prev.expect("every layer from i>=1 in `dp` records its prev_prev");
    }

    let mut matched = Vec::with_capacity(m);
    for (i, (rs, candidates, via_substring)) in placeable.iter().enumerate() {
        let confidence = if *via_substring {
            // Pinned regardless of candidate count or route fit -- a
            // substring match is a guess about identity, not just about
            // which of several confirmed-same-name stops is meant.
            Confidence::Low
        } else if candidates.len() == 1 {
            Confidence::High
        } else {
            let prev = if i > 0 { Some(chosen[i - 1]) } else { None };
            let next = if i + 1 < m { Some(chosen[i + 1]) } else { None };
            let local_cost = |c: u64| -> f64 {
                let mut cost = 0.0;
                if let Some(p) = prev {
                    cost += dist(stops, p, c);
                }
                if let Some(n) = next {
                    cost += dist(stops, c, n);
                }
                if let (Some(p), Some(n)) = (prev, next) {
                    cost += turn_penalty_degrees(stops, p, c, n) * ANGLE_PENALTY_PER_DEGREE_BLOCKS;
                }
                cost
            };
            // The nearest *other* candidate is usually the same stop's
            // opposite-direction twin, a few dozen blocks away at most --
            // comparing costs against it makes the margin look razor-thin
            // even when the pick is obviously right (a real mismatch would
            // be a same-named stop kilometres away in a different
            // neighbourhood, e.g. a common apartment-complex brand name
            // repeated across the city). So the margin check only applies
            // once the alternatives are actually far enough away to matter;
            // a close twin is `Medium` outright, since picking "wrong"
            // there is a small position error either way.
            let nearest_other_distance = candidates
                .iter()
                .filter(|&&c| c != chosen[i])
                .map(|&c| dist(stops, chosen[i], c))
                .fold(f64::MAX, f64::min);
            if nearest_other_distance <= CLOSE_CANDIDATE_THRESHOLD_BLOCKS {
                Confidence::Medium
            } else {
                let chosen_cost = local_cost(chosen[i]).max(1e-6);
                let runner_up = candidates
                    .iter()
                    .filter(|&&c| c != chosen[i])
                    .map(|&c| local_cost(c))
                    .fold(f64::MAX, f64::min);
                if runner_up / chosen_cost >= CONFIDENT_MARGIN_RATIO {
                    Confidence::Medium
                } else {
                    Confidence::Low
                }
            }
        };
        matched.push(MatchedStop {
            seq: rs.seq,
            stop_code: rs.stop_code.clone(),
            stop_name: rs.stop_name.clone(),
            bstopid: chosen[i],
            confidence,
            candidate_count: candidates.len(),
            via_substring: *via_substring,
        });
    }

    let hop_distances: Vec<f64> = (0..m - 1).map(|i| dist(stops, chosen[i], chosen[i + 1])).collect();
    let median = {
        let mut sorted = hop_distances.clone();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let threshold = median * ABNORMAL_HOP_MULTIPLE;
    let abnormal_hops = hop_distances
        .iter()
        .enumerate()
        .filter(|&(_, &d)| d > threshold)
        .map(|(i, &d)| AbnormalHop {
            from_seq: placeable[i].0.seq,
            to_seq: placeable[i + 1].0.seq,
            distance_blocks: d,
            route_median_blocks: median,
        })
        .collect();

    RouteMatchResult { matched, unplaced, abnormal_hops }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(id: u64, name: &str, x: i32, z: i32) -> BusStop {
        BusStop { bstopid: id, ars_no: String::new(), name: name.to_string(), stop_type: "일반".to_string(), lat: 0.0, lon: 0.0, x, z }
    }

    fn route_stop(seq: u32, code: &str, name: &str) -> RouteStop {
        RouteStop { route_no: "508".to_string(), seq, stop_code: code.to_string(), stop_name: name.to_string() }
    }

    #[test]
    fn unique_names_all_resolve_with_high_confidence() {
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "역A", 0, 0));
        stops.insert(2, stop(2, "역B", 10, 0));
        let index = build_name_index(&stops);
        let route = vec![route_stop(0, "c0", "역A"), route_stop(1, "c1", "역B")];
        let result = match_route(&route, &index, &stops);
        assert_eq!(result.matched.len(), 2);
        assert!(result.matched.iter().all(|m| m.confidence == Confidence::High));
        assert!(result.unplaced.is_empty());
    }

    #[test]
    fn picks_the_candidate_that_keeps_the_whole_route_smooth() {
        // A straight line of unambiguous stops, with one ambiguous name in
        // the middle offering a near-collinear candidate and a wildly
        // off-path one. The smooth choice should win even though nothing
        // here is a simple "nearest to one neighbour" case.
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "출발", 0, 0));
        stops.insert(2, stop(2, "갈림", 20, 0)); // collinear, continues straight
        stops.insert(20, stop(20, "갈림", 20, 500)); // wildly off to the side
        stops.insert(3, stop(3, "도착", 40, 0));
        let index = build_name_index(&stops);
        let route = vec![route_stop(0, "c0", "출발"), route_stop(1, "c1", "갈림"), route_stop(2, "c2", "도착")];
        let result = match_route(&route, &index, &stops);
        let middle = result.matched.iter().find(|m| m.stop_name == "갈림").unwrap();
        assert_eq!(middle.bstopid, 2);
    }

    #[test]
    fn a_turnaround_reversal_is_not_penalised_like_a_zigzag() {
        // Outbound then back along the same line: p1(0,0) -> p2(10,0) ->
        // p3(20,0) -> back through p2-ish -> p1-ish. The ambiguous "u1" stop
        // at the far end must resolve to continuing outbound (10,0)-ish
        // is not tested here directly; this test instead checks that a full
        // reversal at the terminus does not get flagged as an abnormal hop
        // by itself (only genuine outliers should be).
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "기점", 0, 0));
        stops.insert(2, stop(2, "중간1", 10, 0));
        stops.insert(3, stop(3, "종점", 20, 0));
        stops.insert(4, stop(4, "중간2", 10, 1)); // slightly offset return-direction stop
        stops.insert(5, stop(5, "복귀", 0, 1));
        let index = build_name_index(&stops);
        let route = vec![
            route_stop(0, "c0", "기점"),
            route_stop(1, "c1", "중간1"),
            route_stop(2, "c2", "종점"),
            route_stop(3, "c3", "중간2"),
            route_stop(4, "c4", "복귀"),
        ];
        let result = match_route(&route, &index, &stops);
        assert_eq!(result.matched.len(), 5);
        // The reversal at the terminus (via 중간1 -> 종점 -> 중간2) should not
        // by itself register as a distance outlier -- all hops here are ~10
        // blocks apart, so none should exceed the abnormal-hop threshold.
        assert!(result.abnormal_hops.is_empty(), "hops: {:?}", result.abnormal_hops.iter().map(|h| h.distance_blocks).collect::<Vec<_>>());
    }

    #[test]
    fn an_isolated_outlier_hop_is_flagged() {
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "A", 0, 0));
        stops.insert(2, stop(2, "B", 10, 0));
        stops.insert(3, stop(3, "C", 20, 0));
        stops.insert(4, stop(4, "D", 2000, 0)); // way out of line with the rest
        let index = build_name_index(&stops);
        let route = vec![
            route_stop(0, "c0", "A"),
            route_stop(1, "c1", "B"),
            route_stop(2, "c2", "C"),
            route_stop(3, "c3", "D"),
        ];
        let result = match_route(&route, &index, &stops);
        assert_eq!(result.abnormal_hops.len(), 1);
        assert_eq!(result.abnormal_hops[0].from_seq, 2);
        assert_eq!(result.abnormal_hops[0].to_seq, 3);
    }

    #[test]
    fn ambiguous_name_with_no_route_context_is_left_unplaced() {
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "겹침", 0, 0));
        stops.insert(2, stop(2, "겹침", 10, 0));
        let index = build_name_index(&stops);
        let route = vec![route_stop(0, "c0", "겹침")];
        let result = match_route(&route, &index, &stops);
        assert!(result.matched.is_empty());
        assert_eq!(result.unplaced.len(), 1);
        assert_eq!(result.unplaced[0].reason, UnplacedReason::AmbiguousNoRouteContext { candidates: 2 });
    }

    #[test]
    fn missing_name_is_reported_distinctly() {
        let stops: HashMap<u64, BusStop> = HashMap::new();
        let index = build_name_index(&stops);
        let route = vec![route_stop(0, "c0", "없는역")];
        let result = match_route(&route, &index, &stops);
        assert_eq!(result.unplaced[0].reason, UnplacedReason::NoNameMatch);
    }

    #[test]
    fn punctuation_and_whitespace_differences_still_match_exactly() {
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "역A", 0, 0));
        stops.insert(2, stop(2, "좌천동가구거리(좌천역)", 10, 0)); // SHP-side spelling
        let index = build_name_index(&stops);
        // CSV-side spelling for the same stop uses "." instead of "()".
        let route = vec![route_stop(0, "c0", "역A"), route_stop(1, "c1", "좌천동가구거리.좌천역")];
        let result = match_route(&route, &index, &stops);
        let hit = result.matched.iter().find(|m| m.seq == 1).expect("should resolve via normalization");
        assert_eq!(hit.bstopid, 2);
        assert!(!hit.via_substring);
        assert_eq!(hit.confidence, Confidence::High); // exactly one normalized match, no choice made
    }

    #[test]
    fn word_order_swap_matches_via_token_set() {
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "역A", 0, 0));
        stops.insert(2, stop(2, "롯데호텔백화점.서면역", 10, 0)); // SHP-side order
        let index = build_name_index(&stops);
        let route = vec![route_stop(0, "c0", "역A"), route_stop(1, "c1", "서면역.롯데호텔백화점")]; // CSV-side, swapped
        let result = match_route(&route, &index, &stops);
        let hit = result.matched.iter().find(|m| m.seq == 1).expect("should resolve via token-set match");
        assert_eq!(hit.bstopid, 2);
        assert!(!hit.via_substring);
    }

    #[test]
    fn substring_fallback_finds_a_real_containment_match() {
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "역A", 0, 0));
        stops.insert(2, stop(2, "동삼혁신지구입구", 10, 0));
        let index = build_name_index(&stops);
        let route = vec![route_stop(0, "c0", "역A"), route_stop(1, "c1", "동삼혁신지구")]; // shorter CSV variant
        let result = match_route(&route, &index, &stops);
        let hit = result.matched.iter().find(|m| m.seq == 1).expect("substring fallback should find it");
        assert_eq!(hit.bstopid, 2);
        assert!(hit.via_substring);
        assert_eq!(hit.confidence, Confidence::Low, "substring matches are always pinned Low");
    }

    #[test]
    fn normalization_widens_candidates_but_route_smoothness_still_decides() {
        // Two stops share a name only after normalization (one written with
        // "." the other with "()"), on opposite sides of the route -- the
        // DP must still pick whichever keeps the path smooth, exactly as it
        // would for an ordinary exact-match ambiguity.
        let mut stops = HashMap::new();
        stops.insert(1, stop(1, "기점", 0, 0));
        stops.insert(2, stop(2, "갈림.역", 10, 0)); // near 기점
        stops.insert(3, stop(3, "갈림(역)", 2000, 0)); // far away
        stops.insert(4, stop(4, "종점", 20, 0));
        let index = build_name_index(&stops);
        let route = vec![
            route_stop(0, "c0", "기점"),
            route_stop(1, "c1", "갈림 역"), // yet another separator variant
            route_stop(2, "c2", "종점"),
        ];
        let result = match_route(&route, &index, &stops);
        let hit = result.matched.iter().find(|m| m.stop_code == "c1").unwrap();
        assert_eq!(hit.bstopid, 2, "route smoothness should pick the near one, not the far one");
        assert!(!hit.via_substring);
    }

    /// One-off diagnostic for route "1011(급행)"'s reported abnormal hops --
    /// is a stop's normalization-widened match wrong, or is this genuinely
    /// an express route with sparse, uneven stop spacing? Not asserting
    /// anything; run with `--ignored --nocapture` to read the printout.
    #[test]
    #[ignore]
    fn diagnose_1011_express_abnormal_hops() {
        let bus_stops_dir = std::path::Path::new("data/부산광역시_버스 정류소 정보(SHP)_20250121");
        let route_csv = std::path::Path::new("data/부산광역시_버스노선별 승하차 정보_20230731.csv");
        let planar = crate::projection::korea_tm::KoreaPlanarBBox::new(0.0, 0.0, 1_000_000.0, 1_000_000.0).unwrap();
        let stops = crate::kr_bus_stops::load_bus_stops(bus_stops_dir, &planar, 1.75).unwrap();
        let route_data = crate::kr_bus_routes::load_route_stops(route_csv).unwrap();
        let index = build_name_index(&stops);
        let route_stops = &route_data.stops_by_route["1011(급행)"];
        let result = match_route(route_stops, &index, &stops);

        println!("=== route 1011(급행): {} CSV stops, {} matched, {} unplaced ===", route_stops.len(), result.matched.len(), result.unplaced.len());
        for (i, m) in result.matched.iter().enumerate() {
            let s = &stops[&m.bstopid];
            let hop_from_prev = if i > 0 {
                let prev = &stops[&result.matched[i - 1].bstopid];
                (((s.x - prev.x).pow(2) + (s.z - prev.z).pow(2)) as f64).sqrt()
            } else {
                0.0
            };
            println!(
                "seq {:>3} \"{}\" ({}) -> bstopid {} pos=({},{}) conf={:?} cands={} substr={} hop_from_prev={:.0}",
                m.seq, m.stop_name, m.stop_code, m.bstopid, s.x, s.z, m.confidence, m.candidate_count, m.via_substring, hop_from_prev
            );
        }
        for h in &result.abnormal_hops {
            println!(
                "[ABNORMAL] seq {} -> {} = {:.0} blocks (median {:.0}, {:.1}x)",
                h.from_seq, h.to_seq, h.distance_blocks, h.route_median_blocks,
                h.distance_blocks / h.route_median_blocks.max(1e-6)
            );
        }
        for u in &result.unplaced {
            println!("[UNPLACED] seq {} \"{}\" ({}) -- {:?}", u.seq, u.stop_name, u.stop_code, u.reason);
        }
    }

    /// One-off: precise EN (EPSG:5186) coordinates for known bridge-adjacent
    /// bus stops, to anchor a MOCT_LINK search for the actual bridge deck
    /// links (SPEC_Bridge §9's own open question -- no bridge field exists,
    /// so this locates them by coordinate instead).
    #[test]
    fn print_bridge_anchor_en_coords() {
        use crate::projection::korea_tm::KoreaTmProjection;
        let points: &[(&str, f64, f64)] = &[
            ("영도대교(mainland)", 35.097058333299998, 129.035885000000007),
            ("영도대교.남포역 A", 35.097318312925999, 129.036016348338990),
            ("영도대교.남포역 B", 35.097618222506000, 129.035894138741014),
            ("대교사거리(yeongdo) A", 35.091462519426003, 129.039461913257014),
            ("대교사거리(yeongdo) B", 35.091687649843998, 129.039220651682001),
            ("부산대교입구", 35.093947115409001, 129.042384558165992),
            ("봉래동교차로(yeongdo)", 35.093409881273999, 129.043695884984004),
            ("중앙동(mainland) A", 35.105587700000001, 129.036016600000011),
            ("중앙동(mainland) B", 35.106248579999999, 129.036372999999998),
        ];
        for &(name, lat, lon) in points {
            let (e, n) = KoreaTmProjection::project_raw(lat, lon);
            println!("{name}: lat={lat} lon={lon} -> E={e:.2} N={n:.2}");
        }
    }
}
