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
//! is what makes the *network solve* safe regardless of whether the terrain
//! pass ran per-tile or on the merged editor, and regardless of region
//! eviction.
//!
//! **Placement is a separate concern from the solve, because of eviction.**
//! A large/borough-scale run always enables `eviction_active`
//! (`data_processing::should_stream_to_disk`): tiles are merged into the
//! main editor and, once a region's owner tile and all 8 neighbours have
//! merged, that region is flushed to disk and dropped from memory. A single
//! post-merge pass writing straight into the main editor -- which is what
//! this module did at first -- runs *after* the tile loop, by which point
//! early regions are already gone; `set_block_absolute` there either drops
//! the write (region flushed) or resurrects an empty region (not flushed
//! yet, but never touched by *this* session, so `save()`'s no-disk-read
//! rule empties it) -- corrupting the exact same way the original
//! reopened-`WorldEditor` bug did, just via a different door. So placement
//! is split from the solve:
//! - [`compute_kr_road_network`] runs once, before the tile loop, needs only
//!   `Ground` (never evicted) and produces plain data (`KrRoadNetwork`) --
//!   no `WorldEditor` involved, so nothing here can be evicted out from
//!   under it.
//! - [`place_segments`] writes blocks for a slice of that data into whatever
//!   `WorldEditor` is handed to it. `data_processing.rs` calls it twice:
//!   once per tile inside the parallel tile closure (each call filtered to
//!   the segments whose bounding box reaches that tile, mirroring how
//!   `tile::assign_elements_to_tiles` already assigns long OSM ways/railways
//!   to *every* tile they intersect) so every write lands before its region
//!   can be evicted; and once, unfiltered, for the small-world sequential
//!   path where there is only ever one non-evicting editor to write into.
//!
//! Future stages (M3 교량, M4 건물, M5 가로 요소) slot in the same way: each
//! is its own function taking `&mut WorldEditor` + whatever inputs it needs,
//! called from `generate_world_with_options` in SPEC_Ingest.md §6's order,
//! all before that one `editor.save()`. None of them should ever construct
//! their own `WorldEditor`; any that places wide/long geometry should follow
//! `place_segments`'s split rather than writing in one post-merge pass.
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
//! - `is_bridge` (SPEC_Build §3.2) is now confirmed, not estimated: `true`
//!   only for `bridges::MANUAL_BRIDGES`'s own hand-built deck segments,
//!   `false` for every 표준노드링크-derived segment -- including any that
//!   physically crosses a bridge this session couldn't manually confirm
//!   (남항대교/부산항대교, and 부산대교's own exact link chain; see
//!   `bridges`' module doc for what was and wasn't confirmed and why).

pub mod bridges;
pub mod routing;
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

/// SPEC_RoadSection.md §2 (2026-09-13 재계산), one field per column of that
/// table's own layout:
///
/// ```text
/// [outer_each][curb_each][carriage_each][median_total][carriage_each][curb_each][outer_each]
/// ```
///
/// `outer_each`/`curb_each`/`carriage_each` are each **one side's** width
/// (both sides equal); `median_total` is the *whole* median, not halved --
/// it is 0 (E/F, no median), 1 (C/D, a single painted centerline), or 4
/// (A/B, a physical divider), matching §1's own units exactly. This makes
/// `total_width()` a direct sum against the table's **총 폭** column, so a
/// test can assert every grade reproduces its documented total instead of
/// trusting the split by construction.
///
/// `outer_is_shoulder` distinguishes A's 갓길 (paved like the carriageway,
/// flush height, no braille/curb-top pattern) from every other grade's real
/// 인도 (raised on the curb, light_gray/andesite pattern) -- SPEC_RoadSection
/// §2's A example still puts a curb column between carriageway and 갓길, so
/// the curb stays; only the outer band's own material/height differs.
struct SectionSpec {
    outer_each: i32,
    curb_each: i32,
    carriage_each: i32,
    median_total: i32,
    outer_is_shoulder: bool,
}

impl SectionSpec {
    fn total_width(&self) -> i32 {
        2 * (self.outer_each + self.curb_each + self.carriage_each) + self.median_total
    }
}

fn section_spec(class: RoadClass) -> SectionSpec {
    match class {
        // 총 50: 갓길4 + 연석1 + 차도18 + 분리대4 + 차도18 + 연석1 + 갓길4
        RoadClass::A => SectionSpec { outer_each: 4, curb_each: 1, carriage_each: 18, median_total: 4, outer_is_shoulder: true },
        // 총 50: 인도4 + 연석1 + 차도18 + 분리대4 + 차도18 + 연석1 + 인도4
        RoadClass::B => SectionSpec { outer_each: 4, curb_each: 1, carriage_each: 18, median_total: 4, outer_is_shoulder: false },
        // 총 31: 인도4 + 연석1 + 차도10 + 중앙선1 + 차도10 + 연석1 + 인도4
        RoadClass::C => SectionSpec { outer_each: 4, curb_each: 1, carriage_each: 10, median_total: 1, outer_is_shoulder: false },
        // 총 21: 인도4 + 연석1 + 차도5 + 중앙선1 + 차도5 + 연석1 + 인도4
        RoadClass::D => SectionSpec { outer_each: 4, curb_each: 1, carriage_each: 5, median_total: 1, outer_is_shoulder: false },
        // 총 20: 인도4 + 연석1 + 차도5 + 차도5 + 연석1 + 인도4 (중앙 없음)
        RoadClass::E => SectionSpec { outer_each: 4, curb_each: 1, carriage_each: 5, median_total: 0, outer_is_shoulder: false },
        // 총 8: 단일 포장면, 인도/연석/중앙 없음
        RoadClass::F => SectionSpec { outer_each: 0, curb_each: 0, carriage_each: 4, median_total: 0, outer_is_shoulder: false },
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
pub struct ProfilePoint {
    x: i32,
    z: i32,
    /// Half-block height unit (SPEC_RoadProfile.md §1.1).
    y2: i32,
}

impl ProfilePoint {
    pub(crate) fn xz(&self) -> (i32, i32) {
        (self.x, self.z)
    }
    /// Half-block unit -- callers wanting a whole-block Y should
    /// `div_euclid(2)` this themselves; kept raw here since some callers
    /// (SPEC_BuildingType.md §10's front-road-height reference) want to know
    /// whether the road sits on a half-block slab, not just its floor.
    pub(crate) fn y2(&self) -> i32 {
        self.y2
    }
}

/// `kr_buildings`' road-occupancy clip (SPEC_Ingest.md §4.2 step 2): the same
/// per-grade total paved width `sweep_and_place` already sweeps, exposed for
/// a caller outside this module.
pub(crate) fn road_total_width(class: RoadClass) -> i32 {
    section_spec(class).total_width()
}

pub struct Segment {
    id: String,
    link_id: String,
    lanes: i32,
    class: RoadClass,
    /// See module doc: `!= "000"` is an unconfirmed heuristic, not a known code table.
    road_type: String,
    f_node: String,
    t_node: String,
    points: Vec<ProfilePoint>,
    /// SPEC_Bridge.md: confirmed, not estimated -- true only for the
    /// hand-built deck segments `bridges::MANUAL_BRIDGES` adds. Every
    /// 표준노드링크-derived segment is `false`, including any that
    /// physically crosses a bridge this session couldn't confirm (see
    /// `bridges`' module doc's disclosed gaps) -- reporting "confirmed
    /// false" there would be as wrong as the old heuristic's guesses, so
    /// those crossings simply aren't flagged yet rather than guessed at.
    is_bridge: bool,
}

impl Segment {
    /// `kr_buildings`' own read side (SPEC_Ingest.md §4.2 / SPEC_RoadSection.md
    /// §5: road-occupancy clipping needs each nearby segment's centerline and
    /// grade, not just its `aabb`). `kr_buildings` is a sibling module, not a
    /// child of this one, so these fields need an explicit accessor the way
    /// `bridges`/`routing` (both children) don't.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn class(&self) -> RoadClass {
        self.class
    }
    pub(crate) fn points(&self) -> &[ProfilePoint] {
        &self.points
    }

    /// `(min_x, max_x, min_z, max_z)` over this segment's centerline points,
    /// padded by its own grade's full cross-section width -- generous enough
    /// that any tile whose (halo-expanded) bounds this overlaps is guaranteed
    /// to cover every block `place_segments` sweeps for it, without needing
    /// to know the tile grid's halo constant here.
    pub fn aabb(&self) -> (i32, i32, i32, i32) {
        let pad = section_spec(self.class).total_width();
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_z = i32::MAX;
        let mut max_z = i32::MIN;
        for p in &self.points {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_z = min_z.min(p.z);
            max_z = max_z.max(p.z);
        }
        (min_x - pad, max_x + pad, min_z - pad, max_z + pad)
    }
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
pub(crate) fn en_to_block(e: f64, n: f64, planar: &KoreaPlanarBBox, scale: f64) -> (f64, f64) {
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
/// Currently unused: both call sites are swapped for `DEBUG_ROAD_HIGHLIGHT`
/// (see that const's doc) and will call back into this once that's reverted.
#[allow(dead_code)]
fn coord_hash(x: i32, z: i32) -> u32 {
    let mut h = (x as i64).wrapping_mul(374_761_393) ^ (z as i64).wrapping_mul(668_265_263);
    h ^= h >> 13;
    (h as u32).wrapping_mul(2_246_822_519)
}

/// One column of `cross_section_layout`'s output: how far from the
/// centerline point (in perpendicular-direction blocks, signed) and which
/// SPEC_RoadSection.md §2 band it falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Band {
    Sidewalk,
    Shoulder,
    Curb,
    Carriage,
    MedianPaint,
    MedianPhysical,
}

/// Expands a `SectionSpec` into left-to-right band + signed-offset pairs
/// covering exactly `total_width()` columns, laid out as SPEC_RoadSection.md
/// §2 writes it:
/// `[outer][curb][carriage][median][carriage][curb][outer]`.
/// Offsets are centered on 0 as evenly as an integer split allows (extra
/// leftover column, when `total_width()` is even, lands just left of
/// center) -- there is no requirement that the two sides mirror exactly to
/// the block, only that the total and each band's own width match §2.
fn cross_section_layout(spec: &SectionSpec) -> Vec<(i32, Band)> {
    let median_band = if spec.median_total == 0 {
        Vec::new()
    } else if spec.median_total == 1 {
        vec![Band::MedianPaint]
    } else {
        vec![Band::MedianPhysical; spec.median_total as usize]
    };
    let outer_band = if spec.outer_is_shoulder { Band::Shoulder } else { Band::Sidewalk };

    let mut sequence = Vec::with_capacity(spec.total_width() as usize);
    sequence.extend(std::iter::repeat(outer_band).take(spec.outer_each as usize));
    sequence.extend(std::iter::repeat(Band::Curb).take(spec.curb_each as usize));
    sequence.extend(std::iter::repeat(Band::Carriage).take(spec.carriage_each as usize));
    sequence.extend(median_band.iter().copied());
    sequence.extend(std::iter::repeat(Band::Carriage).take(spec.carriage_each as usize));
    sequence.extend(std::iter::repeat(Band::Curb).take(spec.curb_each as usize));
    sequence.extend(std::iter::repeat(outer_band).take(spec.outer_each as usize));

    let total = sequence.len() as i32;
    let left_extent = total / 2;
    sequence
        .into_iter()
        .enumerate()
        .map(|(i, band)| (i as i32 - left_extent, band))
        .collect()
}

/// TEMPORARY road-vs-building color collision workaround -- see the two
/// `DEBUG_ROAD_HIGHLIGHT` use sites in `sweep_and_place` for the full
/// rationale and the exact revert. Remove this const and both use sites once
/// `map_renderer.rs`'s preview can tell a road block from a building block on
/// its own (e.g. by consulting `WorldEditor::road_surface_overrides`, which
/// `sweep_and_place` doesn't currently populate either -- see that field's
/// own doc).
const DEBUG_ROAD_HIGHLIGHT: crate::block_definitions::Block = crate::block_definitions::RED_CONCRETE;

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

    for (offset, band) in cross_section_layout(&spec) {
        let px = x + (perp.0 * offset as f64).round() as i32;
        let pz = z + (perp.1 * offset as f64).round() as i32;

        match band {
            Band::Sidewalk => {
                // TEMPORARY (see map_renderer.rs's own doc comment on road-vs-
                // building color collision): the real fix belongs in the map
                // preview renderer, which can't currently tell a road's
                // LIGHT_GRAY_CONCRETE from a building's -- both share the same
                // block palette, so the top-down preview can't distinguish
                // them. Until that's fixed, use a block the KR building
                // palette never touches (kr_buildings/facade.rs) so roads are
                // at least visible in the preview. Revert to the
                // LIGHT_GRAY_CONCRETE/POLISHED_ANDESITE checker pattern this
                // replaced once the renderer is road-aware.
                let block = DEBUG_ROAD_HIGHLIGHT;
                let sidewalk_y2 = y2 + CURB_HEIGHT_Y2;
                place_full(editor, block, px, pz, sidewalk_y2.div_euclid(2));
                if sidewalk_y2.rem_euclid(2) == 1 {
                    place_full(editor, SMOOTH_STONE_SLAB, px, pz, sidewalk_y2.div_euclid(2) + 1);
                }
            }
            Band::Curb => {
                place_full(editor, SMOOTH_STONE_SLAB, px, pz, y_full);
                if has_slab {
                    place_full(editor, SMOOTH_STONE_SLAB, px, pz, y_full + 1);
                }
            }
            Band::MedianPaint => {
                place_full(editor, YELLOW_CONCRETE, px, pz, y_full);
                if has_slab {
                    place_full(editor, YELLOW_CONCRETE, px, pz, y_full + 1);
                }
            }
            Band::MedianPhysical => {
                place_full(editor, STONE_BRICKS, px, pz, y_full);
                if has_slab {
                    place_full(editor, STONE_BRICKS, px, pz, y_full + 1);
                }
            }
            Band::Shoulder | Band::Carriage => {
                // TEMPORARY -- see the Sidewalk arm above; same reason, same
                // revert (the h<3/h<8 BLACK_CONCRETE/LIGHT_GRAY_CONCRETE/
                // GRAY_CONCRETE weathering mix this replaced).
                let block = DEBUG_ROAD_HIGHLIGHT;
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
    curb_width: [i32; 2],
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

/// Everything [`compute_kr_road_network`] solves, in a form that carries no
/// `WorldEditor` reference and is cheap to wrap in `Arc` and share across the
/// parallel tile closures in `data_processing.rs` -- see the module doc for
/// why placement (which does need a `WorldEditor`) is a separate step.
pub struct KrRoadNetwork {
    pub segments: Vec<Segment>,
    pub node_pos: HashMap<String, (i32, i32)>,
    pub node_y2: HashMap<String, i32>,
}

/// Loads, clips, and solves (SPEC_RoadProfile.md P4/P5): produces the full
/// road network as plain data and the counts SPEC_RoadProfile.md §6's
/// automated checks report, but places no blocks and touches no
/// `WorldEditor` -- see the module doc for why. Call once, before the tile
/// loop; feed the result to [`place_segments`] and [`write_roadgraph_json`].
pub fn compute_kr_road_network(
    ground: &Ground,
    xzbbox: &XZBBox,
    planar: &KoreaPlanarBBox,
    scale: f64,
    roads_dir: &Path,
) -> Result<(KrRoadNetwork, KrRoadsReport), String> {
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

    // SPEC_Bridge.md §0: bridges are excluded from the normal ground-
    // following pass and built separately (`bridges::MANUAL_BRIDGES`) --
    // otherwise the real link's terrain-following profile and the bridge's
    // floating deck would both get drawn across the same water crossing.
    // Only `YEONGDO_BRIDGE` has confirmed real LINK_IDs to exclude here
    // (see that module's doc for why 부산대교 and the two harbor bridges
    // don't); this filter is a no-op for the rest until that's known.
    let excluded_link_ids: std::collections::HashSet<&str> =
        bridges::MANUAL_BRIDGES.iter().flat_map(|b| b.excluded_link_ids.iter().copied()).collect();
    let links: Vec<RawLink> = links.into_iter().filter(|l| !excluded_link_ids.contains(l.link_id.as_str())).collect();

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
            is_bridge: false,
        });
    }

    // SPEC_Bridge.md §6.1/§7: manually specified deck segments (see
    // `bridges`' module doc for how -- no confirmed bridge field exists in
    // 표준노드링크). Endpoint height comes from the *already-solved* real
    // node network when the bridge names a confirmed node ID there (so the
    // deck matches the abutment exactly, per §7 "교대 위치를 노드로 고정"),
    // or from a direct `Ground` sample otherwise. Either way, the profile
    // between the two ends is a plain linear interpolation, not
    // `profile_segment`'s ground-following smoothing -- §7's "다리 쪽은
    // 지형 추종을 끄고 매끄러움만 유지" applied literally.
    for (bi, bridge) in bridges::MANUAL_BRIDGES.iter().enumerate() {
        let block_points = resample_to_blocks(
            &bridge.waypoints_en.iter().map(|&(e, n)| en_to_block(e, n, planar, scale)).collect::<Vec<_>>(),
        );
        if block_points.len() < 2 {
            continue;
        }
        let end_y2 = |end: usize| -> f64 {
            if let Some(node_id) = bridge.end_node_ids[end] {
                if let Some(&y2) = node_y2.get(node_id) {
                    return y2 as f64;
                }
            }
            let (x, z) = if end == 0 { block_points[0] } else { block_points[block_points.len() - 1] };
            (ground_y_at(ground, xzbbox, x.round() as i32, z.round() as i32) * 2) as f64
        };
        let start_y2 = end_y2(0);
        let finish_y2 = end_y2(1);

        let cumulative: Vec<f64> = {
            let mut acc = vec![0.0];
            for w in block_points.windows(2) {
                let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
                acc.push(acc.last().unwrap() + d);
            }
            acc
        };
        let total_dist = *cumulative.last().unwrap();
        let points: Vec<ProfilePoint> = block_points
            .iter()
            .zip(cumulative.iter())
            .map(|(&(x, z), &d)| {
                let t = if total_dist > 1e-6 { d / total_dist } else { 0.0 };
                let y2 = (start_y2 + (finish_y2 - start_y2) * t).round() as i32;
                ProfilePoint { x: x.round() as i32, z: z.round() as i32, y2 }
            })
            .collect();

        let lanes = match bridge.class {
            RoadClass::A | RoadClass::B => 6,
            RoadClass::C => 4,
            RoadClass::D | RoadClass::E => 2,
            RoadClass::F => 1,
        };
        segments.push(Segment {
            id: format!("bridge{bi:02}"),
            link_id: format!("manual-bridge-{}", bridge.name),
            lanes,
            class: bridge.class,
            road_type: "bridge".to_string(),
            f_node: bridge.end_node_ids[0].map(str::to_string).unwrap_or_else(|| format!("bridge{bi:02}-a")),
            t_node: bridge.end_node_ids[1].map(str::to_string).unwrap_or_else(|| format!("bridge{bi:02}-b")),
            points,
            is_bridge: true,
        });
    }

    let segments_placed = segments.len();

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

    let report = KrRoadsReport {
        nodes_placed: node_ids.len(),
        segments_placed,
        links_skipped_missing_node,
        slope_violations,
        node_height_mismatches,
    };
    Ok((KrRoadNetwork { segments, node_pos, node_y2 }, report))
}

/// SPEC_RoadProfile.md P6/P7: sweeps and writes blocks for `segments` into
/// `editor`. No save here -- SPEC_Ingest.md §6 step 10 ("월드 쓰기") happens
/// exactly once, in `generate_world_with_options`, after every staged pass
/// (roads now, buildings/street-furniture later) has run.
///
/// Callers decide *which* segments and *which* editor -- see the module doc:
/// the small-world sequential path calls this once with every segment and
/// the single main editor; the parallel tile path calls it once per tile,
/// with only the segments whose [`Segment::aabb`] reaches that tile, against
/// that tile's own `WorldEditor`, before it is merged and its region can be
/// evicted.
pub fn place_segments<'a>(editor: &mut WorldEditor, segments: impl IntoIterator<Item = &'a Segment>) {
    for seg in segments {
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
    }
}

/// SPEC_Build.md §1 "도로 연속성" (road continuity) automated check: unlike
/// `compute_kr_road_network`'s report (slope violations, node height
/// mismatches -- both checked against the *solved height model*, before any
/// block is written), this checks the *actual placed world*: for every
/// centerline point owned by `owned`, was a road/sidewalk-family block really
/// written near its computed height? Must run right after `place_segments`
/// writes into `editor`, in the same call -- never as a later post-merge
/// pass -- for the same eviction reason `place_segments` itself is split per
/// module doc: a flushed region has no disk-read path back.
///
/// `owned` is (min_x, min_z, max_x, max_z), max exclusive, matching
/// `tile::TileBounds`. Points outside it are skipped: the same segment is
/// placed redundantly into every tile its padded aabb overlaps (see the
/// `place_segments` call site), and only the strictly-owning tile's copy
/// survives merge, so checking a halo copy would double-count one tile's
/// point and never check another's. The small-world sequential caller (one
/// editor, no tiles) passes the whole world bbox instead.
///
/// The block set mirrors exactly what `sweep_and_place` writes for every
/// band except the sidewalk (offset 0 -- the centerline point itself -- is
/// always inside the roadway/curb/median bands, never the outer sidewalk
/// band, for every grade in `section_spec`). Returns the unplaced points.
pub fn verify_placement<'a>(
    editor: &WorldEditor,
    segments: impl IntoIterator<Item = &'a Segment>,
    owned: (i32, i32, i32, i32),
) -> Vec<(i32, i32)> {
    use crate::block_definitions::*;
    // Includes DEBUG_ROAD_HIGHLIGHT (RED_CONCRETE) alongside the real palette
    // sweep_and_place normally writes -- see that const's doc. Keep both listed
    // so this check stays correct before and after that temporary swap is
    // reverted, instead of silently false-negatives-ing on whichever isn't there.
    const ROAD_FAMILY: [Block; 8] = [
        GRAY_CONCRETE,
        LIGHT_GRAY_CONCRETE,
        POLISHED_ANDESITE,
        BLACK_CONCRETE,
        SMOOTH_STONE_SLAB,
        STONE_BRICKS,
        YELLOW_CONCRETE,
        DEBUG_ROAD_HIGHLIGHT,
    ];
    let (min_x, min_z, max_x, max_z) = owned;
    let mut unplaced = Vec::new();
    for seg in segments {
        for p in &seg.points {
            if p.x < min_x || p.x >= max_x || p.z < min_z || p.z >= max_z {
                continue;
            }
            let y_full = p.y2.div_euclid(2);
            let found = (-1..=1)
                .any(|dy| editor.get_block_absolute(p.x, y_full + dy, p.z).is_some_and(|b| ROAD_FAMILY.contains(&b)));
            if !found {
                unplaced.push((p.x, p.z));
            }
        }
    }
    unplaced
}

pub fn write_roadgraph_json(
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
            GraphSegment {
                id: s.id.clone(),
                link_id: s.link_id.clone(),
                lanes: s.lanes,
                road_class: s.class.as_str(),
                oneway: false, // see module doc: no confirmed source field
                is_bridge: s.is_bridge, // confirmed, not estimated -- see `Segment::is_bridge`'s own doc
                ends: [s.f_node.clone(), s.t_node.clone()],
                length: s.points.len(),
                points: s
                    .points
                    .iter()
                    .map(|p| GraphPoint {
                        pos: [p.x, p.z],
                        y2: p.y2,
                        road_half_width: [spec.carriage_each, spec.carriage_each],
                        curb_width: [spec.curb_each, spec.curb_each],
                        sidewalk_width: [spec.outer_each, spec.outer_each],
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
    fn section_spec_totals_match_spec_road_section_table() {
        // SPEC_RoadSection.md §2's own **총 폭** column (2026-09-13 재계산).
        let expected = [
            (RoadClass::A, 50),
            (RoadClass::B, 50),
            (RoadClass::C, 31),
            (RoadClass::D, 21),
            (RoadClass::E, 20),
            (RoadClass::F, 8),
        ];
        for (class, total) in expected {
            let spec = section_spec(class);
            assert_eq!(spec.total_width(), total, "{:?} total width", class);
            let layout = cross_section_layout(&spec);
            assert_eq!(layout.len() as i32, total, "{:?} layout column count", class);
        }
    }

    #[test]
    fn cross_section_layout_places_an_explicit_curb_column_where_the_table_says_so() {
        // A/E previously had no curb column at all; §2's own worked examples
        // put one between every carriageway and its outer band except F.
        for class in [RoadClass::A, RoadClass::B, RoadClass::C, RoadClass::D, RoadClass::E] {
            let spec = section_spec(class);
            let layout = cross_section_layout(&spec);
            let curb_columns = layout.iter().filter(|(_, b)| *b == Band::Curb).count();
            assert_eq!(curb_columns, 2, "{:?} must have exactly one curb column on each side", class);
        }
        let f_layout = cross_section_layout(&section_spec(RoadClass::F));
        assert!(f_layout.iter().all(|(_, b)| *b != Band::Curb), "F has no curb");
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
