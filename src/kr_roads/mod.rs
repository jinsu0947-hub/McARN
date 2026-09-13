//! SPEC_Build.md M1 "도로": ingest 표준노드링크 (Korea's national standard
//! node-link road network) directly as a graph -- SPEC_Ingest.md §3.3: "이미
//! 노드-링크 구조다... P1(마스크 추출)과 P2(중심선 추출)는 폐기한다. P3
//! 이후만 남는다." -- then run a scoped-down SPEC_RoadProfile P4-P7 (node
//! height solve, per-segment profile, fixed cross-section sweep, block
//! write) using SPEC_RoadSection §1/§2's fixed per-grade widths, and record
//! the result as `roadgraph.json` (SPEC_Build.md §3.2).
//!
//! SPEC_Ingest.md §6's pipeline ("...4. 지형 생성 -> 5. 도로 종단선형 -> 6.
//! 도로 배치 -> ... -> 10. 월드 쓰기") and §7's own note ("블록 배치·청크
//! 쓰기·월드 저장: 그대로 사용") both say the same thing: this runs inside
//! `data_processing::generate_world_with_options`'s *existing* single
//! `WorldEditor` session, after ground generation and before that session's
//! one `editor.save()` -- never by reopening an already-saved world (that
//! path corrupted terrain: `WorldEditor` has no disk-read path, so its
//! `save()` only ever preserves chunks *this session* touched, replacing
//! everything else with empty filler -- see the git history for the
//! writeup). Reading H0 from `Ground` directly (not from placed blocks)
//! is what makes this safe regardless of whether the terrain pass ran
//! per-tile or on the merged editor, and regardless of region eviction.
//!
//! Future stages (M3 교량, M4 건물, M5 가로 요소) slot in the same way: each
//! is its own function taking `&mut WorldEditor` + whatever inputs it needs,
//! called from `generate_world_with_options` in SPEC_Ingest.md §6's order,
//! all before that one `editor.save()`. None of them should ever construct
//! their own `WorldEditor`.
//!
//! Explicitly out of scope this pass (disclosed, not silently dropped):
//! - SPEC_RoadProfile P3's structure classification (ELEVATED/TUNNEL) reads
//!   air-gap/ceiling clearance from an *already-rendered* road mask; there is
//!   none here (roads are placed directly from vector data, not extracted
//!   from a rendered world). 표준노드링크's fields (§3.1) carry no explicit
//!   bridge/tunnel flag either. Every link is treated as `GROUND`.
//! - P8 (주변 지형 정리: taper/retaining walls) is not implemented.
//! - SPEC_RoadSection §3 (도색/횡단보도), §4 (교차로 연석 반경), §6.5
//!   (중앙분리대 조경) and street furniture are excluded per this task's own
//!   scope, not an oversight.
//! - `oneway` (SPEC_Build §3.2) has no confirmed source field in the actual
//!   `.dbf` (see `shapefile.rs`'s field dump) and is always recorded `false`.
//! - `is_bridge` (SPEC_Build §3.2) is approximated as `ROAD_TYPE != "000"`
//!   (the sampled records' apparent "normal road" code) -- not a confirmed
//!   code table, so treat this field as low-confidence.

pub mod shapefile;

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::projection::korea_tm::KoreaPlanarBBox;
use crate::world_editor::WorldEditor;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use shapefile::DbfTable;

/// SPEC_RoadProfile.md §1.2 / §4: 45 degrees, expressed as max |Δy2| per block.
const MAX_SLOPE_Y2: i32 = 2;
/// SPEC_RoadSection.md §1: y2 = 1 (half a block).
const CURB_HEIGHT_Y2: i32 = 1;
/// Iteration counts for the two Gauss-Seidel-style relaxations (§4's
/// `MAX_ITER`=10 is for the *constraint* repair loop specifically; the
/// smoothing relaxations themselves need many more sweeps to converge on a
/// graph this size and are cheap enough that a fixed generous count is
/// simpler than a convergence-delta check).
const NODE_SOLVE_SWEEPS: usize = 300;
const NODE_SLOPE_REPAIR_PASSES: usize = 50;
const PROFILE_SMOOTH_SWEEPS: usize = 40;
const PROFILE_SLOPE_REPAIR_PASSES: usize = 20;
/// SPEC_RoadProfile.md §4 `w_slope` ("조정 필요"): weights the node solve's
/// slope-penalty term against the ground-follow term. Chosen, not measured --
/// large enough that intersections visibly flatten relative to raw terrain,
/// small enough that a node many hops from a slope conflict still tracks
/// its own ground height.
const W_SLOPE: f64 = 4.0;
/// SPEC_RoadProfile.md §4 `λ_curve` ("조정 필요"): iteration count stands in
/// for the weight here (see `PROFILE_SMOOTH_SWEEPS`) -- more sweeps of plain
/// Laplacian averaging is qualitatively the same knob as a larger λ in the
/// least-squares form (both trade ground-fidelity for smoothness).

/// One 표준노드링크 node, kept only as (id, EN metres) -- no attributes are
/// consumed from `MOCT_NODE.dbf` beyond `NODE_ID` for the join key.
struct RawNode {
    id: String,
    e: f64,
    n: f64,
}

/// One link with its full centerline, still in EN metres. Endpoints are the
/// polyline's own first/last vertex, which SPEC_Ingest.md §3.1 assumes
/// coincide with `F_NODE`/`T_NODE`'s node geometry (표준노드링크's own
/// invariant, not re-verified here beyond the join succeeding).
struct RawLink {
    link_id: String,
    f_node: String,
    t_node: String,
    lanes: i32,
    road_rank: String,
    road_name: String,
    road_type: String,
    points_en: Vec<(f64, f64)>,
}

/// SPEC_Ingest.md §3.2 cross-section grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadClass {
    A,
    B,
    C,
    D,
    E,
    F,
}

impl RoadClass {
    fn as_str(self) -> &'static str {
        match self {
            RoadClass::A => "A",
            RoadClass::B => "B",
            RoadClass::C => "C",
            RoadClass::D => "D",
            RoadClass::E => "E",
            RoadClass::F => "F",
        }
    }
}

/// SPEC_RoadSection.md §2's six grades, reduced to what the sweep (P6/P7)
/// needs: total half-width from the centerline, how many of those blocks
/// (at the outer edge) are sidewalk, and whether a curb course separates
/// them from the roadway. Widths are each grade's own `총 폭 / 2` (rounded
/// down; C/D's odd total gives the left side the extra block, §
/// `cross_section` for exactly where) -- SPEC_RoadSection.md's own worked
/// example (D: `[인도4][연석][차도5][중앙선1][차도5][연석][인도4]`) and its
/// summary table's per-column widths do not reduce to the same sub-split
/// (curb/median accounting differs between the two), so this only commits to
/// matching the table's headline **총 폭**, the number the spec repeats in
/// every row and the one this task's automated checks can verify against
/// the placed blocks. Sub-splitting inside that total is this module's own
/// choice, disclosed here rather than presented as a spec-derived value.
struct SectionSpec {
    half_total: i32,
    sidewalk_each: i32,
    has_curb: bool,
    has_median_tint: bool,
}

fn section_spec(class: RoadClass) -> SectionSpec {
    match class {
        RoadClass::A => SectionSpec { half_total: 24, sidewalk_each: 0, has_curb: false, has_median_tint: true },
        RoadClass::B => SectionSpec { half_total: 26, sidewalk_each: 4, has_curb: true, has_median_tint: true },
        RoadClass::C => SectionSpec { half_total: 16, sidewalk_each: 4, has_curb: true, has_median_tint: true },
        RoadClass::D => SectionSpec { half_total: 10, sidewalk_each: 4, has_curb: true, has_median_tint: true },
        RoadClass::E => SectionSpec { half_total: 8, sidewalk_each: 3, has_curb: false, has_median_tint: false },
        RoadClass::F => SectionSpec { half_total: 4, sidewalk_each: 0, has_curb: false, has_median_tint: false },
    }
}

/// SPEC_Ingest.md §3.2's mapping table, `LANES`-overrides-`ROAD_RANK` rule
/// applied exactly as written: "**`LANES`가 등급을 덮어쓴다.** 등급이 104여도
/// 차로수가 2면 C가 아니라 D로 내린다" -- the spec's own worked example, taken
/// literally: `LANES` can push a link *below* its rank row's own listed pair
/// (104's row lists only `{B, C}`, yet 2 lanes gives `D`), not just choose
/// between the two options the row names.
///
/// **Disclosed gap**: the spec gives worked examples for `104` (this one)
/// but not for `107` (`{D, E}`) or for `108`/unrecognised ranks (the
/// separate LANES-only table, itself ambiguous at `2 이상`/`6 이상`). Those
/// two are this module's own choice, not spec-derived: `107` reads `D` at 2+
/// lanes and `E` below that (mirrors 104's "more lanes wins the higher of
/// its two options" shape); an unranked/`108` link defaults to the
/// *narrower* of an ambiguous pair (`B` not `A`, `E` not `D`) -- guessing
/// narrow on an unclassified road is the conservative direction to be wrong.
fn road_class(road_rank: &str, lanes: i32) -> RoadClass {
    match road_rank {
        "101" | "102" => RoadClass::A,
        "103" => RoadClass::B,
        "104" => {
            if lanes >= 6 {
                RoadClass::B
            } else if lanes == 4 {
                RoadClass::C
            } else {
                RoadClass::D // spec's own example: 104 + 2 lanes -> D, not C
            }
        }
        "105" | "106" => RoadClass::C,
        "107" => {
            if lanes >= 2 {
                RoadClass::D
            } else {
                RoadClass::E
            }
        }
        _ => match lanes {
            l if l >= 6 => RoadClass::B,
            4 => RoadClass::C,
            2 => RoadClass::E,
            _ => RoadClass::F, // 1 or missing/zero
        },
    }
}

/// One resampled centerline point, ~1 per Minecraft block, carrying enough
/// to write `roadgraph.json`'s `segments[].points[]` and to sweep P6/P7.
struct ProfilePoint {
    x: i32,
    z: i32,
    /// Half-block height unit (SPEC_RoadProfile.md §1.1).
    y2: i32,
}

struct Segment {
    id: String,
    link_id: String,
    lanes: i32,
    class: RoadClass,
    /// See module doc: `!= "000"` is an unconfirmed heuristic, not a known code table.
    road_type: String,
    f_node: String,
    t_node: String,
    points: Vec<ProfilePoint>,
}

/// What the CLI/report prints after a run -- SPEC_RoadProfile.md §6's
/// regression checks, run for real against the placed geometry rather than
/// assumed from the solver's own constraints.
pub struct KrRoadsReport {
    pub nodes_placed: usize,
    pub segments_placed: usize,
    pub links_skipped_missing_node: usize,
    pub slope_violations: usize,
    pub node_height_mismatches: usize,
}

/// Verifies the `.dbf` field names this module reads are actually present,
/// before touching a single record -- SPEC_Ingest.md §3.1's own footnote
/// ("필드명은 배포본에 따라 다를 수 있으므로 실제 파일에서 확인한다") made
/// concrete as a startup check instead of a one-off manual read.
fn verify_fields(link_dbf: &DbfTable, node_dbf: &DbfTable) -> Result<(), String> {
    let required_link = ["LINK_ID", "F_NODE", "T_NODE", "LANES", "ROAD_RANK", "ROAD_NAME"];
    let link_fields = link_dbf.field_names();
    for f in required_link {
        if !link_fields.contains(&f) {
            return Err(format!(
                "MOCT_LINK.dbf is missing field {f} (spec: SPEC_Ingest.md §3.1). Actual fields: {link_fields:?}"
            ));
        }
    }
    let required_node = ["NODE_ID"];
    let node_fields = node_dbf.field_names();
    for f in required_node {
        if !node_fields.contains(&f) {
            return Err(format!(
                "MOCT_NODE.dbf is missing field {f}. Actual fields: {node_fields:?}"
            ));
        }
    }
    Ok(())
}

fn load_raw(dir: &Path) -> Result<(Vec<RawNode>, Vec<RawLink>), String> {
    let node_dbf = DbfTable::open(&dir.join("MOCT_NODE.dbf")).map_err(|e| format!("MOCT_NODE.dbf: {e}"))?;
    let link_dbf = DbfTable::open(&dir.join("MOCT_LINK.dbf")).map_err(|e| format!("MOCT_LINK.dbf: {e}"))?;
    verify_fields(&link_dbf, &node_dbf)?;

    let node_points = shapefile::read_points(&dir.join("MOCT_NODE.shp")).map_err(|e| format!("MOCT_NODE.shp: {e}"))?;
    shapefile::assert_row_counts_match(&node_dbf, node_points.len(), "MOCT_NODE").map_err(|e| e.to_string())?;
    let mut nodes = Vec::with_capacity(node_points.len());
    for (i, (e, n)) in node_points.into_iter().enumerate() {
        nodes.push(RawNode {
            id: node_dbf.get(i, "NODE_ID"),
            e,
            n,
        });
    }

    let link_lines =
        shapefile::read_polylines(&dir.join("MOCT_LINK.shp")).map_err(|e| format!("MOCT_LINK.shp: {e}"))?;
    shapefile::assert_row_counts_match(&link_dbf, link_lines.len(), "MOCT_LINK").map_err(|e| e.to_string())?;
    let mut links = Vec::with_capacity(link_lines.len());
    for (i, points_en) in link_lines.into_iter().enumerate() {
        if points_en.len() < 2 {
            continue;
        }
        let lanes: i32 = link_dbf.get(i, "LANES").parse().unwrap_or(0);
        links.push(RawLink {
            link_id: link_dbf.get(i, "LINK_ID"),
            f_node: link_dbf.get(i, "F_NODE"),
            t_node: link_dbf.get(i, "T_NODE"),
            lanes,
            road_rank: link_dbf.get(i, "ROAD_RANK"),
            road_name: link_dbf.get(i, "ROAD_NAME"),
            road_type: link_dbf.get(i, "ROAD_TYPE"),
            points_en,
        });
    }

    Ok((nodes, links))
}

/// SPEC_Ingest.md §3's own scoping note ("전국 배포본이므로 부산 영도·중구·
/// 동구 일대만 잘라 쓴다") applied as a plain EN-metres bounding-box filter:
/// this run's own generation bbox already sits inside that area, so filtering
/// to it both scopes the load and is the only clip precision this task's
/// generation actually needs.
fn clip(nodes: Vec<RawNode>, links: Vec<RawLink>, en_bbox: (f64, f64, f64, f64)) -> (Vec<RawNode>, Vec<RawLink>) {
    let (e_min, n_min, e_max, n_max) = en_bbox;
    let in_bbox = |e: f64, n: f64| e >= e_min && e <= e_max && n >= n_min && n <= n_max;

    let links: Vec<RawLink> = links
        .into_iter()
        .filter(|l| l.points_en.iter().any(|&(e, n)| in_bbox(e, n)))
        .collect();

    let mut needed_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for l in &links {
        needed_ids.insert(l.f_node.as_str());
        needed_ids.insert(l.t_node.as_str());
    }
    let nodes: Vec<RawNode> = nodes.into_iter().filter(|n| needed_ids.contains(n.id.as_str())).collect();
    (nodes, links)
}

/// Converts EN metres to Minecraft block coordinates by SPEC_Ingest.md
/// §2.2's own formula, reusing this run's already-resolved origin -- the
/// same transform `main.rs` uses for everything else, so road geometry lands
/// exactly where the rest of the world expects it.
fn en_to_block(e: f64, n: f64, planar: &KoreaPlanarBBox, scale: f64) -> (f64, f64) {
    let x = (e - planar.e_min()) * scale;
    let z = (planar.n_min() - n) * scale; // north = -Z (per §2.2)
    // n_min was the *south* edge in the original formula's convention (E0/N0
    // is the SW corner), but this task's `planar` is `KoreaPlanarBBox` (an
    // e/n envelope), whose `n_min` is the geometric minimum -- i.e. also the
    // south edge. Consistent with `main.rs`'s own e0/n0 usage.
    (x, z)
}

/// Terrain height (Minecraft Y) at this world block column, read straight
/// from `Ground`'s own elevation grid -- the same source
/// `ground_generation`/`highways.rs` place blocks from -- rather than by
/// probing already-placed blocks. `Ground::level` already carries M0's
/// §2.3 elevation-compression result (that is what it *is*, downstream of
/// `scale_to_minecraft`), so this is not a second, inconsistent view of
/// elevation, and unlike probing placed blocks it works identically whether
/// the terrain pass ran per-tile or on the merged editor, and regardless of
/// which regions a large run may have since evicted.
///
/// `ground.level` indexes relative to the world's own origin (0, 0), not
/// world block coordinates directly -- SPEC_Ingest.md's Kr origin usually
/// makes `xzbbox.min_x() == 0`, but `min_z()` is negative (north = -Z), so
/// both axes are offset here rather than assuming only one needs it.
fn ground_y_at(ground: &Ground, xzbbox: &XZBBox, x: i32, z: i32) -> i32 {
    ground.level(XZPoint::new(x - xzbbox.min_x(), z - xzbbox.min_z()))
}

/// SPEC_RoadProfile.md P4: one global relaxation over the whole node graph.
/// Gauss-Seidel sweeps toward the weighted-least-squares optimum (ground
/// term + inverse-distance-weighted neighbour term), then a separate
/// constraint-repair pass pulls any edge still over `MAX_SLOPE_Y2` together --
/// a fixed-point iteration standing in for §4's "선형 최소자승 ... 위반 시
/// 페널티 가중 재계산", which needs an actual sparse solver this module does
/// not have one of.
fn solve_node_heights(
    node_ids: &[String],
    h0_y2: &HashMap<String, i32>,
    edges: &[(String, String, f64)], // (a, b, distance_in_blocks)
) -> HashMap<String, f64> {
    let mut adjacency: HashMap<&str, Vec<(&str, f64)>> = HashMap::new();
    for (a, b, d) in edges {
        adjacency.entry(a.as_str()).or_default().push((b.as_str(), *d));
        adjacency.entry(b.as_str()).or_default().push((a.as_str(), *d));
    }

    let mut h: HashMap<&str, f64> = node_ids
        .iter()
        .map(|id| (id.as_str(), *h0_y2.get(id).unwrap_or(&0) as f64))
        .collect();

    for _ in 0..NODE_SOLVE_SWEEPS {
        for id in node_ids {
            let Some(neighbours) = adjacency.get(id.as_str()) else {
                continue;
            };
            let h0 = *h0_y2.get(id).unwrap_or(&0) as f64;
            let mut num = h0; // w_ground = 1.0 for every GROUND node (§4)
            let mut den = 1.0;
            for &(m, d) in neighbours {
                let w = W_SLOPE / d.max(1.0);
                num += w * h[m];
                den += w;
            }
            h.insert(id.as_str(), num / den);
        }
    }

    // Constraint repair: pull the two ends of any still-too-steep edge together.
    for _ in 0..NODE_SLOPE_REPAIR_PASSES {
        let mut worst = 0.0_f64;
        for (a, b, d) in edges {
            let ha = h[a.as_str()];
            let hb = h[b.as_str()];
            let limit = MAX_SLOPE_Y2 as f64 * d;
            let diff = ha - hb;
            if diff.abs() > limit {
                let excess = diff.abs() - limit;
                worst = worst.max(excess);
                let shift = excess / 2.0 * diff.signum();
                h.insert(a.as_str(), ha - shift);
                h.insert(b.as_str(), hb + shift);
            }
        }
        if worst < 1e-6 {
            break;
        }
    }

    node_ids.iter().map(|id| (id.clone(), h[id.as_str()])).collect()
}

/// SPEC_RoadProfile.md P5, scoped down: Laplacian smoothing (repeated
/// 3-point averaging with the endpoints held fixed) approximates the
/// curvature-penalty least-squares fit λ_curve describes -- both trade
/// ground-fidelity for smoothness, and iterating a simple average toward a
/// smoother curve is the direct fixed-point analogue of minimising 2nd-
/// difference squared, without solving the pentadiagonal normal equations
/// exactly. Endpoints are seeded from the already-solved node heights so
/// the two never drift apart during smoothing.
fn profile_segment(h0_y2: &[i32], start_y2: f64, end_y2: f64) -> Vec<i32> {
    let n = h0_y2.len();
    if n == 1 {
        return vec![start_y2.round() as i32];
    }
    let mut h: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            h0_y2[i] as f64 + (1.0 - t) * (start_y2 - h0_y2[0] as f64) + t * (end_y2 - h0_y2[n - 1] as f64)
        })
        .collect();
    h[0] = start_y2;
    h[n - 1] = end_y2;

    for _ in 0..PROFILE_SMOOTH_SWEEPS {
        let snapshot = h.clone();
        for i in 1..n - 1 {
            h[i] = (snapshot[i - 1] + snapshot[i] + snapshot[i + 1]) / 3.0;
        }
    }

    for _ in 0..PROFILE_SLOPE_REPAIR_PASSES {
        let mut changed = false;
        for i in 1..n {
            let diff = h[i] - h[i - 1];
            if diff.abs() > MAX_SLOPE_Y2 as f64 {
                let excess = diff.abs() - MAX_SLOPE_Y2 as f64;
                let shift = excess / 2.0 * diff.signum();
                if i > 1 {
                    h[i - 1] += shift;
                } else {
                    h[i] -= shift; // can't move the fixed start; absorb into the next point
                }
                if i < n - 1 {
                    h[i] -= shift;
                } else {
                    h[i - 1] += shift;
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    h[0] = start_y2;
    h[n - 1] = end_y2;

    // Integerise last (SPEC_RoadProfile.md P5 "정수화"): round, then a final
    // clamp pass so rounding itself cannot reintroduce a slope violation.
    let mut y2: Vec<i32> = h.iter().map(|v| v.round() as i32).collect();
    for _ in 0..PROFILE_SLOPE_REPAIR_PASSES {
        let mut changed = false;
        for i in 1..n {
            let diff = y2[i] - y2[i - 1];
            if diff.abs() > MAX_SLOPE_Y2 {
                let sign = diff.signum();
                y2[i] = y2[i - 1] + sign * MAX_SLOPE_Y2;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    y2[0] = start_y2.round() as i32;
    y2
}

/// Resamples a polyline (in block coordinates) to ~1 point per block --
/// SPEC_RoadProfile.md §1.2 defines slope per block, so the profile solver
/// and the automated slope check both need points at that spacing, not at
/// whatever vertex spacing the source polyline happens to have.
fn resample_to_blocks(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = vec![points[0]];
    for w in points.windows(2) {
        let (x0, z0) = w[0];
        let (x1, z1) = w[1];
        let dist = ((x1 - x0).powi(2) + (z1 - z0).powi(2)).sqrt();
        let steps = dist.round().max(1.0) as usize;
        for s in 1..=steps {
            let t = s as f64 / steps as f64;
            out.push((x0 + (x1 - x0) * t, z0 + (z1 - z0) * t));
        }
    }
    out
}

/// Deterministic per-column hash for the material variation SPEC_RoadSection
/// §6.1 requires to be reproducible ("반드시 좌표 해시로 결정") -- reused
/// verbatim from that requirement's own reasoning, not Arnis's RNG.
fn coord_hash(x: i32, z: i32) -> u32 {
    let mut h = (x as i64).wrapping_mul(374_761_393) ^ (z as i64).wrapping_mul(668_265_263);
    h ^= h >> 13;
    (h as u32).wrapping_mul(2_246_822_519)
}

/// SPEC_RoadProfile.md P6/P7 + SPEC_RoadSection.md §1/§2, combined: sweeps
/// the fixed cross-section for `class` across the perpendicular at `point`,
/// writing roadway/curb/sidewalk blocks at `y2`. `dir` is the (unit-ish)
/// forward direction, used only to build the perpendicular.
#[allow(clippy::too_many_arguments)]
fn sweep_and_place(editor: &mut WorldEditor, x: i32, z: i32, y2: i32, class: RoadClass, dir: (f64, f64)) {
    use crate::block_definitions::*;

    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-6);
    let perp = (-dir.1 / len, dir.0 / len);
    let spec = section_spec(class);

    let y_full = y2.div_euclid(2);
    let has_slab = y2.rem_euclid(2) == 1;

    let place_full = |editor: &mut WorldEditor, block: crate::block_definitions::Block, px: i32, pz: i32, y: i32| {
        editor.set_block_absolute(block, px, y, pz, None, None);
    };

    for side in [-1i32, 1i32] {
        for offset in 0..=spec.half_total {
            let px = x + (perp.0 * (offset * side) as f64).round() as i32;
            let pz = z + (perp.1 * (offset * side) as f64).round() as i32;

            let from_edge = spec.half_total - offset;
            let is_sidewalk = from_edge < spec.sidewalk_each;
            let is_curb = spec.has_curb && from_edge == spec.sidewalk_each;
            let is_median = spec.has_median_tint && offset == 0;

            if is_sidewalk {
                let block = if coord_hash(px, pz) % 2 == 0 { LIGHT_GRAY_CONCRETE } else { POLISHED_ANDESITE };
                let sidewalk_y2 = y2 + CURB_HEIGHT_Y2;
                place_full(editor, block, px, pz, sidewalk_y2.div_euclid(2));
                if sidewalk_y2.rem_euclid(2) == 1 {
                    place_full(editor, SMOOTH_STONE_SLAB, px, pz, sidewalk_y2.div_euclid(2) + 1);
                }
            } else if is_curb {
                place_full(editor, SMOOTH_STONE_SLAB, px, pz, y_full);
                if has_slab {
                    place_full(editor, SMOOTH_STONE_SLAB, px, pz, y_full + 1);
                }
            } else {
                let h = coord_hash(px, pz) % 100;
                let block = if is_median {
                    STONE_BRICKS
                } else if h < 3 {
                    BLACK_CONCRETE
                } else if h < 8 {
                    LIGHT_GRAY_CONCRETE
                } else {
                    GRAY_CONCRETE
                };
                place_full(editor, block, px, pz, y_full);
                if has_slab {
                    place_full(editor, block, px, pz, y_full + 1);
                }
            }
        }
    }
}

#[derive(Serialize)]
struct GraphNode {
    id: String,
    pos: [i32; 2],
    y2: i32,
    degree: usize,
    segments: Vec<String>,
}

#[derive(Serialize)]
struct GraphPoint {
    pos: [i32; 2],
    y2: i32,
    road_half_width: [i32; 2],
    sidewalk_width: [i32; 2],
    structure: &'static str,
}

#[derive(Serialize)]
struct GraphSegment {
    id: String,
    link_id: String,
    lanes: i32,
    road_class: &'static str,
    oneway: bool,
    is_bridge: bool,
    ends: [String; 2],
    length: usize,
    points: Vec<GraphPoint>,
}

#[derive(Serialize)]
struct RoadGraph {
    version: &'static str,
    bbox: [i32; 4],
    nodes: Vec<GraphNode>,
    segments: Vec<GraphSegment>,
    excluded: Vec<serde_json::Value>,
}

/// Entry point: loads, clips, solves, sweeps, writes `roadgraph.json`, and
/// returns the counts SPEC_RoadProfile.md §6's automated checks report.
/// `world_dir` must already contain M0's generated terrain -- this reopens
/// it with a fresh `WorldEditor` rather than participating in the tile-
/// parallel terrain generation pass, since road placement needs the *final*
/// ground surface as its P4/P5 input (§ `ground_y_at`), not a tile's
/// in-progress one.
pub fn generate_kr_roads(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    planar: &KoreaPlanarBBox,
    scale: f64,
    roads_dir: &Path,
    graph_output_dir: &Path,
) -> Result<KrRoadsReport, String> {
    let (raw_nodes, raw_links) = load_raw(roads_dir)?;
    println!(
        "KR roads: loaded {} nodes, {} links from {}",
        raw_nodes.len(),
        raw_links.len(),
        roads_dir.display()
    );

    let en_bbox = (planar.e_min(), planar.n_min(), planar.e_max(), planar.n_max());
    let (nodes, links) = clip(raw_nodes, raw_links, en_bbox);
    println!("KR roads: clipped to this run's bbox -> {} nodes, {} links", nodes.len(), links.len());
    // ROAD_NAME (SPEC_Ingest.md §3.1: "표기·확인용") is not consumed by the
    // solver -- its one use is confirming, by eye, that the clip actually
    // caught the streets this run's bbox is supposed to cover.
    let mut distinct_names: Vec<&str> = links
        .iter()
        .map(|l| l.road_name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    distinct_names.sort_unstable();
    distinct_names.dedup();
    if !distinct_names.is_empty() {
        println!("KR roads: named streets in this clip: {}", distinct_names.join(", "));
    }

    let mut node_pos: HashMap<String, (i32, i32)> = HashMap::new();
    for n in &nodes {
        let (bx, bz) = en_to_block(n.e, n.n, planar, scale);
        node_pos.insert(n.id.clone(), (bx.round() as i32, bz.round() as i32));
    }

    // H0 per node: the terrain surface `Ground` already computed at that node's block position.
    let mut h0_y2: HashMap<String, i32> = HashMap::new();
    let mut links_skipped_missing_node = 0usize;
    let mut usable_links: Vec<&RawLink> = Vec::new();
    for l in &links {
        let (Some(&(fx, fz)), Some(&(tx, tz))) = (node_pos.get(&l.f_node), node_pos.get(&l.t_node)) else {
            links_skipped_missing_node += 1;
            continue;
        };
        for (id, x, z) in [(&l.f_node, fx, fz), (&l.t_node, tx, tz)] {
            h0_y2.entry(id.clone()).or_insert_with(|| ground_y_at(ground, xzbbox, x, z) * 2);
        }
        usable_links.push(l);
    }

    let node_ids: Vec<String> = h0_y2.keys().cloned().collect();
    let mut edges: Vec<(String, String, f64)> = Vec::new();
    for l in &usable_links {
        let (fx, fz) = node_pos[&l.f_node];
        let (tx, tz) = node_pos[&l.t_node];
        let dist = (((fx - tx).pow(2) + (fz - tz).pow(2)) as f64).sqrt().max(1.0);
        edges.push((l.f_node.clone(), l.t_node.clone(), dist));
    }
    let solved = solve_node_heights(&node_ids, &h0_y2, &edges);
    let node_y2: HashMap<String, i32> = solved.iter().map(|(id, h)| (id.clone(), h.round() as i32)).collect();

    let mut segments: Vec<Segment> = Vec::new();
    for (i, l) in usable_links.iter().enumerate() {
        let block_points = resample_to_blocks(
            &l.points_en
                .iter()
                .map(|&(e, n)| en_to_block(e, n, planar, scale))
                .collect::<Vec<_>>(),
        );
        if block_points.len() < 2 {
            continue;
        }
        let h0_along: Vec<i32> = block_points
            .iter()
            .map(|&(x, z)| ground_y_at(ground, xzbbox, x.round() as i32, z.round() as i32) * 2)
            .collect();
        let start_y2 = node_y2[&l.f_node] as f64;
        let end_y2 = node_y2[&l.t_node] as f64;
        let y2_profile = profile_segment(&h0_along, start_y2, end_y2);

        let class = road_class(&l.road_rank, l.lanes);
        let points: Vec<ProfilePoint> = block_points
            .iter()
            .zip(y2_profile.iter())
            .map(|(&(x, z), &y2)| ProfilePoint { x: x.round() as i32, z: z.round() as i32, y2 })
            .collect();

        segments.push(Segment {
            id: format!("s{:05}", i),
            link_id: l.link_id.clone(),
            lanes: l.lanes,
            class,
            road_type: l.road_type.clone(),
            f_node: l.f_node.clone(),
            t_node: l.t_node.clone(),
            points,
        });
    }

    // P6/P7: sweep and write blocks into the *same* session's editor. No
    // save here -- SPEC_Ingest.md §6 step 10 ("월드 쓰기") happens exactly
    // once, in `generate_world_with_options`, after every staged pass
    // (roads now, buildings/street-furniture later) has run.
    let mut segments_placed = 0usize;
    for seg in &segments {
        for w in seg.points.windows(2) {
            let dir = ((w[1].x - w[0].x) as f64, (w[1].z - w[0].z) as f64);
            sweep_and_place(editor, w[0].x, w[0].z, w[0].y2, seg.class, dir);
        }
        if let Some(last) = seg.points.last() {
            let dir = if seg.points.len() >= 2 {
                let p = &seg.points[seg.points.len() - 2];
                ((last.x - p.x) as f64, (last.z - p.z) as f64)
            } else {
                (1.0, 0.0)
            };
            sweep_and_place(editor, last.x, last.z, last.y2, seg.class, dir);
        }
        segments_placed += 1;
    }

    // Automated checks (SPEC_RoadProfile.md §6 "3번은 회귀 검사로 매 실행 후 자동 수행한다"),
    // run against the actual placed integer y2 values, not assumed from the solver's own constraints.
    let mut slope_violations = 0usize;
    for seg in &segments {
        for w in seg.points.windows(2) {
            if (w[1].y2 - w[0].y2).abs() > MAX_SLOPE_Y2 {
                slope_violations += 1;
            }
        }
    }
    let mut node_height_mismatches = 0usize;
    let mut node_seen_y2: HashMap<&str, i32> = HashMap::new();
    for seg in &segments {
        for (node_id, y2) in [
            (seg.f_node.as_str(), seg.points.first().map(|p| p.y2)),
            (seg.t_node.as_str(), seg.points.last().map(|p| p.y2)),
        ] {
            let Some(y2) = y2 else { continue };
            match node_seen_y2.get(node_id) {
                Some(&prev) if prev != y2 => node_height_mismatches += 1,
                _ => {
                    node_seen_y2.insert(node_id, y2);
                }
            }
        }
    }

    write_roadgraph_json(graph_output_dir, xzbbox, &segments, &node_pos, &node_y2)?;

    Ok(KrRoadsReport {
        nodes_placed: node_ids.len(),
        segments_placed,
        links_skipped_missing_node,
        slope_violations,
        node_height_mismatches,
    })
}

fn write_roadgraph_json(
    dir: &Path,
    xzbbox: &XZBBox,
    segments: &[Segment],
    node_pos: &HashMap<String, (i32, i32)>,
    node_y2: &HashMap<String, i32>,
) -> Result<(), String> {
    let mut node_segments: HashMap<String, Vec<String>> = HashMap::new();
    for s in segments {
        node_segments.entry(s.f_node.clone()).or_default().push(s.id.clone());
        node_segments.entry(s.t_node.clone()).or_default().push(s.id.clone());
    }

    let nodes: Vec<GraphNode> = node_y2
        .keys()
        .filter_map(|id| {
            let &(x, z) = node_pos.get(id)?;
            let segs = node_segments.get(id).cloned().unwrap_or_default();
            Some(GraphNode {
                id: id.clone(),
                pos: [x, z],
                y2: node_y2[id],
                degree: segs.len(),
                segments: segs,
            })
        })
        .collect();

    let graph_segments: Vec<GraphSegment> = segments
        .iter()
        .map(|s| {
            let spec = section_spec(s.class);
            let road_half = spec.half_total - spec.sidewalk_each - i32::from(spec.has_curb);
            GraphSegment {
                id: s.id.clone(),
                link_id: s.link_id.clone(),
                lanes: s.lanes,
                road_class: s.class.as_str(),
                oneway: false, // see module doc: no confirmed source field
                is_bridge: !s.road_type.is_empty() && s.road_type != "000", // see module doc: unconfirmed heuristic
                ends: [s.f_node.clone(), s.t_node.clone()],
                length: s.points.len(),
                points: s
                    .points
                    .iter()
                    .map(|p| GraphPoint {
                        pos: [p.x, p.z],
                        y2: p.y2,
                        road_half_width: [road_half, road_half],
                        sidewalk_width: [spec.sidewalk_each, spec.sidewalk_each],
                        structure: "ground",
                    })
                    .collect(),
            }
        })
        .collect();

    let graph = RoadGraph {
        version: "0.1",
        bbox: [xzbbox.min_x(), xzbbox.min_z(), xzbbox.max_x(), xzbbox.max_z()],
        nodes,
        segments: graph_segments,
        excluded: Vec::new(),
    };

    let text = serde_json::to_string_pretty(&graph).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("roadgraph.json"), text).map_err(|e| e.to_string())
}

/// The two required grade-mapping/road-class free functions, unit-testable
/// without a shapefile or a `WorldEditor` -- everything else in this module
/// needs a real world/graph to exercise meaningfully.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_101_102_is_a_regardless_of_lanes() {
        assert_eq!(road_class("101", 2), RoadClass::A);
        assert_eq!(road_class("102", 8), RoadClass::A);
    }

    #[test]
    fn lanes_overrides_rank_104() {
        // "등급이 104여도 차로수가 2면 C가 아니라 D로 내린다"
        assert_eq!(road_class("104", 2), RoadClass::D);
        assert_eq!(road_class("104", 6), RoadClass::B);
    }

    #[test]
    fn rank_107_splits_on_lanes() {
        // 107's own row is {D, E}; this module's disclosed tiebreak (see
        // `road_class`) picks D at 2+ lanes, E below that.
        assert_eq!(road_class("107", 4), RoadClass::D);
        assert_eq!(road_class("107", 2), RoadClass::D);
        assert_eq!(road_class("107", 1), RoadClass::E);
        assert_eq!(road_class("107", 0), RoadClass::E);
    }

    #[test]
    fn rank_104_at_four_lanes_is_c() {
        assert_eq!(road_class("104", 4), RoadClass::C);
        assert_eq!(road_class("104", 0), RoadClass::D); // no lanes on record: same floor as 2
    }

    #[test]
    fn unknown_rank_falls_back_to_the_narrower_lanes_default() {
        assert_eq!(road_class("999", 4), RoadClass::C); // unambiguous at 4 lanes
        assert_eq!(road_class("999", 6), RoadClass::B); // ambiguous A-or-B -> narrower default B
        assert_eq!(road_class("999", 2), RoadClass::E); // ambiguous D-or-E -> narrower default E
        assert_eq!(road_class("999", 0), RoadClass::F);
        assert_eq!(road_class("108", 1), RoadClass::F);
    }

    #[test]
    fn node_solve_keeps_a_flat_graph_flat() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let h0: HashMap<String, i32> = ids.iter().map(|i| (i.clone(), 20)).collect();
        let edges = vec![("a".to_string(), "b".to_string(), 10.0), ("b".to_string(), "c".to_string(), 10.0)];
        let solved = solve_node_heights(&ids, &h0, &edges);
        for id in &ids {
            assert!((solved[id] - 20.0).abs() < 1e-6, "flat input must stay flat, got {:?}", solved[id]);
        }
    }

    #[test]
    fn node_solve_repairs_a_cliff_between_neighbours() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let mut h0 = HashMap::new();
        h0.insert("a".to_string(), 0);
        h0.insert("b".to_string(), 100); // way more than MAX_SLOPE_Y2 * distance
        let edges = vec![("a".to_string(), "b".to_string(), 5.0)];
        let solved = solve_node_heights(&ids, &h0, &edges);
        let diff = (solved["a"] - solved["b"]).abs();
        assert!(
            diff <= MAX_SLOPE_Y2 as f64 * 5.0 + 1e-6,
            "node solve must respect MAX_SLOPE, got diff {diff}"
        );
    }

    #[test]
    fn profile_segment_matches_fixed_endpoints_and_respects_slope() {
        let h0: Vec<i32> = vec![0, 50, 0, 50, 0, 50, 0, 50, 0, 50];
        let y2 = profile_segment(&h0, 0.0, 4.0);
        assert_eq!(y2[0], 0);
        assert_eq!(*y2.last().unwrap(), 4);
        for w in y2.windows(2) {
            assert!((w[1] - w[0]).abs() <= MAX_SLOPE_Y2, "profile must respect MAX_SLOPE: {y2:?}");
        }
    }
}
