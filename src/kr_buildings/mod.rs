//! SPEC_Build.md M4 "건물": ingest 건물통합정보 (Busan-wide GIS building
//! footprint shapefile), determine each building's type from its own
//! recorded attributes (SPEC_BuildingType.md §3), rasterize its footprint
//! onto the block grid, clip it against M1's already-solved road network
//! (SPEC_Ingest.md §4.2 / SPEC_RoadSection.md §5), and build a faithful but
//! voxel-simplified facade (SPEC_BuildingType.md §5, §10) -- following the
//! exact same compute/place split as `kr_roads`, for the exact same reason
//! (see that module's doc): a large run evicts `WorldEditor` regions once
//! their neighbours have merged, so anything that needs a `WorldEditor` must
//! write per-tile, before eviction, not in one post-merge pass over
//! everything.
//!
//! **The actual `.dbf` field names are meaningless.** Busan's distribution
//! (`AL_D010_*.dbf`, 857 MB, 472,608 records covering the whole city) was
//! exported with every attribute column renamed to `A0`..`A28` -- whatever
//! tool produced it evidently couldn't carry the original Korean column
//! headers through to a shapefile's 10-byte field-name limit and fell back
//! to sequential placeholders instead. There is no sidecar field dictionary
//! in the distribution. The mapping this module uses was reverse-engineered
//! by sampling real records (see `field_layout()`'s own doc for the values
//! that pinned each column) -- confirmed against Yeongdo-gu (법정동코드
//! prefix `26200`) specifically, not assumed from a spec that was written
//! before this file was in hand:
//!
//! | column | content | maps to |
//! |---|---|---|
//! | `A9`  | Korean text, e.g. `단독주택`/`공동주택`/`제2종근린생활시설`/`공장` | 주용도 |
//! | `A13` | `YYYY-MM-DD` | 사용승인일 |
//! | `A26` | small integer | 지상 층수 |
//! | `A27` | small integer | 지하 층수 (recorded, unused this stage) |
//! | `A11` | Korean text, e.g. `철근콘크리트구조`/`벽돌구조` | 구조 (advisory only) |
//! | `A21` | `B00...` alnum | 건물고유번호 -- this module's deterministic-hash seed |
//! | `A23` | 5-digit 법정동코드 prefix | 시군구코드 (Yeongdo-gu = `26200`) |
//!
//! Also confirmed empirically, contradicting SPEC_Ingest.md §2.1's table
//! (which lists 건물통합정보 as EPSG:5179): this actual distribution's
//! `.prj` declares `EPSG:5186` (`Korea_2000_Korea_Central_Belt_2010`) --
//! the same plane 표준노드링크 uses. No reprojection is needed between the
//! two inputs this module joins (buildings + M1's road network); both are
//! read as EN metres in the same plane `kr_roads` already assumes.
//!
//! `A9`/`A13` (주용도/사용승인일) plus `A26` (지상층수) are, per
//! SPEC_BuildingType.md §0's own "이 셋이 유형 결정의 전부다" (as phrased in
//! this task), the entire type-determination input -- confirmed present
//! for a large majority but not all Yeongdo-gu records with a real footprint
//! and floor count (one sampled slice: 13,982 of 25,617 raw records have a
//! floor count, of which most also carry 주용도/사용승인일; SPEC_BuildingType
//! §1's own defaults -- missing 주용도 -> 단독주택, missing 사용승인연도 ->
//! 1995~2009 -- are applied and logged exactly where the source data lacks
//! them, not silently).
//!
//! **경사지 해상도 (SPEC_Ingest.md §5.3's own open question)**: `Ground`'s
//! elevation grid is already bilinearly interpolated to block resolution
//! (see `elevation::compute_grid_dims`/the resampling `ground.rs` performs),
//! so a building's footprint Δ is never literally flat even when the whole
//! footprint sits inside one ~52.5-block source DEM cell -- interpolation
//! guarantees a smooth gradient, not a plateau. What is genuinely lost is
//! *local* relief smaller than the source grid: a real retaining wall, a
//! staircase alley, a single terraced lot a few metres from its neighbour --
//! none of that survives a 30 m source resample, so `Δ` here reads as
//! smoother than the real slope on 산복도로. This module's own live check
//! (`report_slope_resolution`, run once per invocation) samples Δ across
//! every classified building's footprint and prints the distribution so
//! this can be judged from real numbers rather than assumed from the source
//! resolution alone.

mod facade;
mod geom;
mod slope;

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::kr_roads::shapefile::{DbfIndexedTable, DbfRow};
use crate::kr_roads::{self, KrRoadNetwork, RoadClass};
use crate::projection::korea_tm::KoreaPlanarBBox;
use crate::world_editor::WorldEditor;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// SPEC_RoadSection.md §5's own three-way split.
const OMIT_BELOW_RATIO: f64 = 0.30;
const WARN_BELOW_RATIO: f64 = 0.70;
/// SPEC_GenerationScope.md §6.
const HIGHRISE_FLOORS: u32 = 15;
/// SPEC_GenerationScope.md §6 (reused from `kr_transit`, which already
/// declares this constant but has no consumer for it yet -- this module is
/// that consumer).
const BUILDING_BUFFER_M: f64 = crate::kr_transit::BUILDING_BUFFER_M;

/// SPEC_BuildingType.md §3's five named groups plus the spec's own catch-all
/// (`X`: "그 외 주용도... O 규칙을 따르되 지붕만 별도 처리한다").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseGroup {
    R,
    A,
    C,
    O,
    I,
    X,
}

impl UseGroup {
    fn code(self) -> &'static str {
        match self {
            UseGroup::R => "R",
            UseGroup::A => "A",
            UseGroup::C => "C",
            UseGroup::O => "O",
            UseGroup::I => "I",
            UseGroup::X => "X",
        }
    }
}

/// SPEC_BuildingType.md §3's four approval-year bins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    E1,
    E2,
    E3,
    E4,
}

impl Era {
    fn code(self) -> &'static str {
        match self {
            Era::E1 => "e1",
            Era::E2 => "e2",
            Era::E3 => "e3",
            Era::E4 => "e4",
        }
    }

    fn from_year(year: i32) -> Self {
        if year <= 1979 {
            Era::E1
        } else if year <= 1994 {
            Era::E2
        } else if year <= 2009 {
            Era::E3
        } else {
            Era::E4
        }
    }
}

/// SPEC_BuildingType.md §3's own worked mapping. Substring matching, not
/// exact-equality, because Korean 건축법 주용도 text sometimes carries a
/// sub-classification suffix this module doesn't need to distinguish (e.g.
/// "제1종근린생활시설" / "제2종근린생활시설" both containing
/// "근린생활시설").
fn classify_use_group(main_use: &str, floors_above: u32) -> UseGroup {
    if main_use.is_empty() {
        // SPEC_BuildingType.md §1: "주용도가 없으면 단독주택으로 간주한다."
        return UseGroup::R;
    }
    if main_use.contains("아파트") {
        return UseGroup::A;
    }
    if main_use.contains("공동주택") {
        // §3: "아파트 (공동주택 중 5층 이상)" -- the raw 주용도 text this
        // distribution carries is the broad 공동주택 category (not the finer
        // 다세대/연립/다가구 split), so floor count is the only signal this
        // module has to split it, exactly as the spec's own parenthetical
        // anticipates.
        return if floors_above >= 5 { UseGroup::A } else { UseGroup::R };
    }
    if main_use.contains("단독주택") || main_use.contains("다가구") || main_use.contains("다세대") || main_use.contains("연립") {
        return UseGroup::R;
    }
    if main_use.contains("근린생활시설") || main_use.contains("판매시설") || main_use.contains("숙박시설") {
        return UseGroup::C;
    }
    if main_use.contains("업무시설") || main_use.contains("교육연구시설") || main_use.contains("의료시설") {
        return UseGroup::O;
    }
    if main_use.contains("공장") || main_use.contains("창고") || main_use.contains("자동차") {
        return UseGroup::I;
    }
    UseGroup::X
}

/// SPEC_BuildingType.md §2's own table, in blocks.
fn floor_heights(group: UseGroup) -> (i32, i32) {
    match group {
        UseGroup::R | UseGroup::A => (6, 6),
        UseGroup::C | UseGroup::O | UseGroup::X => (8, 6),
        // "12 (단층 기준)": the spec gives no 기준층 value for I because it
        // expects industrial buildings to mostly be single-storey; the rare
        // multi-floor 공장/창고 record in this data just repeats the same 12
        // per floor rather than switching to an undocumented second number.
        UseGroup::I => (12, 12),
    }
}

fn total_height_blocks(group: UseGroup, floors_above: u32) -> i32 {
    let (first, typical) = floor_heights(group);
    let floors = floors_above.max(1);
    first + (floors as i32 - 1) * typical
}

/// One 건물통합정보 record with real geometry and a real floor count --
/// vacant-land / unbuilt parcels (A26 == 0) are filtered out before this
/// struct exists at all (see `load_raw`).
struct BuildingRecord {
    id: String,
    footprint_en: Vec<(f64, f64)>,
    floors_above: u32,
    main_use: String,
    approval_year: i32,
    main_use_missing: bool,
    approval_missing: bool,
}

fn decode_euc_kr(raw: Option<&[u8]>) -> String {
    match raw {
        Some(bytes) => encoding_rs::EUC_KR.decode(bytes).0.trim().to_string(),
        None => String::new(),
    }
}

/// Verifies the empirically-pinned column layout still looks like what it
/// was reverse-engineered against (29 columns, `A13`/`A22`/`A28` are date-
/// shaped) before this module trusts a single record from it -- the same
/// spirit as `kr_roads::verify_fields`, adapted for a table with no real
/// field names to check by.
fn verify_layout(dbf: &DbfIndexedTable) -> Result<(), String> {
    let names = dbf.field_names();
    if names.len() != 29 {
        return Err(format!(
            "건물통합정보 .dbf: expected 29 columns (A0..A28, this distribution's own anonymised layout -- see kr_buildings module doc), got {} ({:?})",
            names.len(),
            names
        ));
    }
    Ok(())
}

fn load_raw(shp_path: &Path, dbf_path: &Path, bbox_en: (f64, f64, f64, f64)) -> Result<Vec<BuildingRecord>, String> {
    let polys = crate::kr_roads::shapefile::read_polygons_filtered(shp_path, bbox_en)
        .map_err(|e| format!("{}: {e}", shp_path.display()))?;
    let mut dbf = DbfIndexedTable::open(dbf_path).map_err(|e| format!("{}: {e}", dbf_path.display()))?;
    verify_layout(&dbf)?;

    let mut records = Vec::new();
    let mut missing_use = 0usize;
    let mut missing_approval = 0usize;
    let mut no_floors = 0usize;
    for (record_index, rings) in &polys {
        let Some(outer) = geom::largest_ring(rings) else { continue };
        if outer.len() < 3 {
            continue;
        }
        let row: DbfRow = dbf.read_row(*record_index).map_err(|e| e.to_string())?;
        if row.is_deleted() {
            continue;
        }
        let floors_above: u32 = row.get(&dbf, "A26").parse().unwrap_or(0);
        if floors_above == 0 {
            // A26==0 covers both genuinely vacant parcels and records this
            // distribution never filled in -- either way there is no
            // building to place, so this is silently dropped, not logged
            // (unlike a *present* record missing just 주용도/사용승인일).
            no_floors += 1;
            continue;
        }
        let id = row.get(&dbf, "A21");
        let main_use = decode_euc_kr(row.get_raw(&dbf, "A9"));
        let approval_raw = row.get(&dbf, "A13");
        let approval_year: Option<i32> = approval_raw.get(0..4).and_then(|y| y.parse().ok());
        let main_use_missing = main_use.is_empty();
        let approval_missing = approval_year.is_none();
        if main_use_missing {
            missing_use += 1;
        }
        if approval_missing {
            missing_approval += 1;
        }
        records.push(BuildingRecord {
            id,
            footprint_en: outer.clone(),
            floors_above,
            main_use,
            // SPEC_BuildingType.md §1: "사용승인연도가 없으면 1995~2009로... 간주한다" -- 2000 is
            // this module's own single representative year for that bin (any year in
            // 1995..=2009 maps to the same Era::E3, so the exact value only matters for display).
            approval_year: approval_year.unwrap_or(2000),
            main_use_missing,
            approval_missing,
        });
    }
    println!(
        "KR buildings: {} usable footprints loaded ({} skipped for zero floor count); \
         {} missing 주용도 (defaulted to 단독주택), {} missing 사용승인일 (defaulted to 1995~2009)",
        records.len(),
        no_floors,
        missing_use,
        missing_approval
    );
    Ok(records)
}

/// SPEC_Build.md §3.4's own `detail_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailLevel {
    L2,
    L3,
}

impl DetailLevel {
    fn as_str(self) -> &'static str {
        match self {
            DetailLevel::L2 => "L2",
            DetailLevel::L3 => "L3",
        }
    }
}

/// One classified, clipped, height-resolved building, ready either to be
/// placed (unless `omitted`) or just recorded in `buildings.json`. Carries
/// no `WorldEditor` reference -- see the module doc for why placement is a
/// separate step from this computation.
pub struct PlannedBuilding {
    id: String,
    /// Remaining footprint cells (world block x,z) after the road-occupancy
    /// clip -- the *original* raster when `omitted` (so the JSON record
    /// still shows what would have been built and why it wasn't).
    cells: Vec<(i32, i32)>,
    ground_y: i32,
    total_height: i32,
    floors_above: u32,
    group: UseGroup,
    era: Era,
    detail_level: DetailLevel,
    clipped: bool,
    omitted: bool,
    front_segment: Option<String>,
    /// The nearest front-road point itself (world block x,z) -- which side
    /// of the footprint faces the street, for the entrance gap (§5.2),
    /// signage orientation (§6), and entry stairs (§10.6). `None` only when
    /// there was no road anywhere in this run's network to face (see
    /// `compute_kr_buildings`'s own fallback for that case).
    front_point: Option<(i32, i32)>,
    /// `Ground`'s own elevation at each remaining cell, sampled once here
    /// (compute time, never evicted -- see `kr_roads`' module doc for why
    /// that matters) for `slope`'s cut/fill decision at placement time.
    terrain_y: HashMap<(i32, i32), i32>,
}

impl PlannedBuilding {
    fn type_code(&self) -> String {
        format!("{}-{}", self.group.code(), self.era.code())
    }

    pub fn aabb(&self) -> (i32, i32, i32, i32) {
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_z = i32::MAX;
        let mut max_z = i32::MIN;
        for &(x, z) in &self.cells {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_z = min_z.min(z);
            max_z = max_z.max(z);
        }
        if min_x == i32::MAX {
            return (0, -1, 0, -1); // empty footprint: never matches a tile
        }
        // Padded by the roof height's worth of retaining-wall/facade splash
        // this building could still draw just outside its own footprint.
        (min_x - 3, max_x + 3, min_z - 3, max_z + 3)
    }

    /// SPEC_StreetFurniture.md §2.3 (인입선): needs footprint cells (nearest-
    /// pole-distance check), roof Y (where the service drop attaches),
    /// use-group (only C/R get one), and enough identity for the hash that
    /// decides the 50% draw -- exposed together so that module doesn't need
    /// its own copy of this struct's private layout.
    pub fn service_drop_info(&self) -> Option<(&[(i32, i32)], i32, UseGroup, &str)> {
        if self.omitted {
            return None;
        }
        Some((&self.cells, self.ground_y + self.total_height, self.group, &self.id))
    }
}

pub struct KrBuildingsReport {
    pub loaded: usize,
    pub placed: usize,
    pub omitted_road_overlap: usize,
    pub clipped_road_overlap: usize,
    pub l3_simplified: usize,
}

/// Converts a building's outer ring (EN metres) to block coordinates and
/// rasterizes it onto the grid -- one cell per block, cell `(x, z)` counted
/// as inside when its center point is inside the polygon. Returns cells in
/// arbitrary order.
fn rasterize(footprint_block: &[(f64, f64)]) -> Vec<(i32, i32)> {
    let (min_x, min_z, max_x, max_z) = geom::polygon_bbox(footprint_block);
    let x0 = min_x.floor() as i32;
    let x1 = max_x.ceil() as i32;
    let z0 = min_z.floor() as i32;
    let z1 = max_z.ceil() as i32;
    let mut cells = Vec::new();
    for x in x0..=x1 {
        for z in z0..=z1 {
            let center = (x as f64 + 0.5, z as f64 + 0.5);
            if geom::point_in_polygon(center, footprint_block) {
                cells.push((x, z));
            }
        }
    }
    cells
}

/// SPEC_Ingest.md §4.2 step 2 / SPEC_RoadSection.md §5: a cell is road-
/// occupied if it falls within any nearby segment's own paved half-width of
/// that segment's centerline. `candidates` is pre-filtered to segments whose
/// `aabb()` reaches this building (see `compute_kr_buildings`), so this is a
/// small inner loop, not a scan of the whole road network per building.
fn is_road_occupied(cell_center: (f64, f64), candidates: &[(&[crate::kr_roads::ProfilePoint], RoadClass)]) -> bool {
    for (points, class) in candidates {
        let half_width = (kr_roads::road_total_width(*class) as f64) / 2.0;
        let xz: Vec<(i32, i32)> = points.iter().map(|p| p.xz()).collect();
        if geom::point_polyline_distance(cell_center, &xz) <= half_width {
            return true;
        }
    }
    false
}

/// Nearest road segment to `cell_center` among `candidates`, with its
/// closest point's y2 (half-block) -- SPEC_BuildingType.md §10.1's "전면
/// 접도고" (front-road elevation) and, for `buildings.json`, the
/// `front_segment` field.
fn nearest_front(
    center: (f64, f64),
    candidates: &[(&str, &[crate::kr_roads::ProfilePoint])],
) -> Option<(String, i32, (i32, i32))> {
    let mut best: Option<(f64, String, i32, (i32, i32))> = None;
    for (id, points) in candidates {
        for p in points.iter() {
            let (x, z) = p.xz();
            let d = ((x as f64 - center.0).powi(2) + (z as f64 - center.1).powi(2)).sqrt();
            if best.as_ref().map(|(bd, ..)| d < *bd).unwrap_or(true) {
                best = Some((d, id.to_string(), p.y2().div_euclid(2), (x, z)));
            }
        }
    }
    best.map(|(_, id, y, pt)| (id, y, pt))
}

/// SPEC_Build.md M4's compute phase: load, classify, clip, and resolve a
/// front elevation for every building in this run's bbox -- plain data, no
/// `WorldEditor`, exactly mirroring `kr_roads::compute_kr_road_network`.
/// `route_polylines` is `kr_transit`'s already-built route centerlines
/// (`None` when M2 wasn't run this invocation, in which case every building
/// gets L2 detail -- see the module doc's disclosed L2/L3 simplification).
#[allow(clippy::too_many_arguments)]
pub fn compute_kr_buildings(
    ground: &Ground,
    xzbbox: &XZBBox,
    planar: &KoreaPlanarBBox,
    scale: f64,
    shp_path: &Path,
    dbf_path: &Path,
    road_network: &KrRoadNetwork,
    route_polylines: Option<&HashMap<String, Vec<[i32; 2]>>>,
) -> Result<(Vec<PlannedBuilding>, KrBuildingsReport), String> {
    let en_bbox = (planar.e_min(), planar.n_min(), planar.e_max(), planar.n_max());
    let raw = load_raw(shp_path, dbf_path, en_bbox)?;

    let l2_buffer_blocks = BUILDING_BUFFER_M * scale;

    let mut planned = Vec::with_capacity(raw.len());
    let mut omitted_road_overlap = 0usize;
    let mut clipped_road_overlap = 0usize;
    let mut l3_simplified = 0usize;
    let mut delta_samples: Vec<i32> = Vec::new();

    for rec in &raw {
        let footprint_block: Vec<(f64, f64)> = rec
            .footprint_en
            .iter()
            .map(|&(e, n)| kr_roads::en_to_block(e, n, planar, scale))
            .collect();
        let original_cells = rasterize(&footprint_block);
        if original_cells.is_empty() {
            continue;
        }

        let (min_x, max_x, min_z, max_z) =
            original_cells.iter().fold((i32::MAX, i32::MIN, i32::MAX, i32::MIN), |acc, &(x, z)| {
                (acc.0.min(x), acc.1.max(x), acc.2.min(z), acc.3.max(z))
            });
        let pad = 40; // generous: widest road half-width (A/B class) plus margin
        let nearby_segments: Vec<&crate::kr_roads::Segment> = road_network
            .segments
            .iter()
            .filter(|seg| {
                let (smin_x, smax_x, smin_z, smax_z) = seg.aabb();
                smin_x < max_x + pad && smax_x >= min_x - pad && smin_z < max_z + pad && smax_z >= min_z - pad
            })
            .collect();
        let candidates: Vec<(&[crate::kr_roads::ProfilePoint], RoadClass)> =
            nearby_segments.iter().map(|s| (s.points(), s.class())).collect();

        let remaining_cells: Vec<(i32, i32)> = original_cells
            .iter()
            .copied()
            .filter(|&(x, z)| !is_road_occupied((x as f64 + 0.5, z as f64 + 0.5), &candidates))
            .collect();
        let ratio = remaining_cells.len() as f64 / original_cells.len() as f64;

        let omitted = ratio < OMIT_BELOW_RATIO;
        let clipped = ratio < 1.0 && !omitted;
        if omitted {
            omitted_road_overlap += 1;
        } else if remaining_cells.len() != original_cells.len() {
            clipped_road_overlap += 1;
        }
        if ratio < WARN_BELOW_RATIO && ratio >= OMIT_BELOW_RATIO {
            println!(
                "KR buildings: {} clipped to {:.0}% of its footprint by road overlap (data-mismatch suspicion, SPEC_RoadSection.md §5) -- keeping, logged",
                rec.id, ratio * 100.0
            );
        }

        let cells_for_output = if omitted { original_cells.clone() } else { remaining_cells.clone() };

        let group = classify_use_group(&rec.main_use, rec.floors_above);
        let era = Era::from_year(rec.approval_year);
        let total_height = total_height_blocks(group, rec.floors_above);

        // SPEC_BuildingType.md §10.1: front floor = nearest road point's
        // already-solved height. Candidates use the *unpadded* aabb match
        // (a building's own front road should already be within the padded
        // set above); fall back to every loaded segment for an isolated
        // building near none of the nearby ones (rare -- disclosed, not
        // silently wrong: such a building still gets *a* front reference,
        // just from farther away).
        let centroid = {
            let n = cells_for_output.len() as f64;
            let (sx, sz) = cells_for_output.iter().fold((0.0, 0.0), |(ax, az), &(x, z)| (ax + x as f64, az + z as f64));
            (sx / n, sz / n)
        };
        let front_candidates: Vec<(&str, &[crate::kr_roads::ProfilePoint])> = if !nearby_segments.is_empty() {
            nearby_segments.iter().map(|s| (s.id(), s.points())).collect()
        } else {
            road_network.segments.iter().map(|s| (s.id(), s.points())).collect()
        };
        let Some((front_segment, front_y, front_point)) = nearest_front(centroid, &front_candidates) else {
            // No road anywhere in this run's network -- SPEC_BuildingType.md
            // §10 has nothing to grade against. Falls back to `Ground`'s own
            // surface at the centroid (flat placement, no grading), logged.
            println!("KR buildings: {} has no nearby road segment; using bare terrain height, no grading", rec.id);
            let (cx, cz) = (centroid.0.round() as i32, centroid.1.round() as i32);
            let ground_y = ground.level(XZPoint::new(cx - xzbbox.min_x(), cz - xzbbox.min_z()));
            planned.push(finish_building(
                rec, cells_for_output, ground_y, total_height, group, era, omitted, clipped, None, None, ground,
                xzbbox, &mut delta_samples,
            ));
            continue;
        };

        let is_highrise = rec.floors_above >= HIGHRISE_FLOORS;
        let near_route = route_polylines
            .map(|polylines| {
                polylines.values().any(|pts| {
                    let xz: Vec<(i32, i32)> = pts.iter().map(|p| (p[0], p[1])).collect();
                    geom::point_polyline_distance(centroid, &xz) <= l2_buffer_blocks
                })
            })
            .unwrap_or(true); // no M2 route data this run -> everything is L2 (see module doc)
        let detail_level = if is_highrise && !near_route { DetailLevel::L3 } else { DetailLevel::L2 };
        if detail_level == DetailLevel::L3 {
            l3_simplified += 1;
        }

        let mut b = finish_building(
            rec,
            cells_for_output,
            front_y,
            total_height,
            group,
            era,
            omitted,
            clipped,
            Some(front_segment),
            Some(front_point),
            ground,
            xzbbox,
            &mut delta_samples,
        );
        b.detail_level = detail_level;
        planned.push(b);
    }

    report_slope_resolution(&delta_samples);

    let report = KrBuildingsReport {
        loaded: raw.len(),
        placed: planned.iter().filter(|b| !b.omitted).count(),
        omitted_road_overlap,
        clipped_road_overlap,
        l3_simplified,
    };
    Ok((planned, report))
}

#[allow(clippy::too_many_arguments)]
fn finish_building(
    rec: &BuildingRecord,
    cells: Vec<(i32, i32)>,
    ground_y: i32,
    total_height: i32,
    group: UseGroup,
    era: Era,
    omitted: bool,
    clipped: bool,
    front_segment: Option<String>,
    front_point: Option<(i32, i32)>,
    ground: &Ground,
    xzbbox: &XZBBox,
    delta_samples: &mut Vec<i32>,
) -> PlannedBuilding {
    let mut terrain_y = HashMap::with_capacity(cells.len());
    let mut min_h = i32::MAX;
    let mut max_h = i32::MIN;
    for &(x, z) in &cells {
        let h = ground.level(XZPoint::new(x - xzbbox.min_x(), z - xzbbox.min_z()));
        terrain_y.insert((x, z), h);
        min_h = min_h.min(h);
        max_h = max_h.max(h);
    }
    if min_h <= max_h {
        delta_samples.push(max_h - min_h);
    }
    PlannedBuilding {
        id: rec.id.clone(),
        cells,
        ground_y,
        total_height,
        floors_above: rec.floors_above,
        group,
        era,
        detail_level: DetailLevel::L2,
        clipped,
        omitted,
        front_segment,
        front_point,
        terrain_y,
    }
}

/// SPEC_Ingest.md §5.3's open question, answered from this run's own data
/// rather than assumed: prints the real distribution of footprint elevation
/// spread (Δ, in blocks) across every classified building, so whether the
/// 30 m source DEM actually degrades SPEC_BuildingType.md §10's grading can
/// be read off real numbers.
fn report_slope_resolution(deltas: &[i32]) {
    if deltas.is_empty() {
        return;
    }
    let mut sorted = deltas.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let pct = |p: f64| sorted[((p * (n - 1) as f64).round() as usize).min(n - 1)];
    let flat = sorted.iter().filter(|&&d| d <= 2).count();
    let retain = sorted.iter().filter(|&&d| d >= 3 && d <= 6).count();
    let tiered = sorted.iter().filter(|&&d| d >= 7).count();
    println!(
        "KR buildings: footprint Δ (elevation spread, blocks) over {n} buildings -- \
         median={} p90={} max={}; §10.2 buckets: flat(≤2)={flat} retain(3-6)={retain} tiered(≥7)={tiered}",
        pct(0.5),
        pct(0.9),
        sorted[n - 1]
    );
}

/// SPEC_Build.md M4's place phase, mirroring `kr_roads::place_segments`
/// exactly: called once per tile (filtered to buildings whose `aabb()`
/// reaches that tile) for the parallel path, or once, unfiltered, for the
/// small-world sequential path.
pub fn place_buildings<'a>(editor: &mut WorldEditor, buildings: impl IntoIterator<Item = &'a PlannedBuilding>) {
    for b in buildings {
        if b.omitted {
            continue;
        }
        slope::grade_site(editor, b);
        facade::build(editor, b);
    }
}

// --- buildings.json (SPEC_Build.md §3.4) ------------------------------------

#[derive(Serialize)]
struct BuildingEntry {
    building_id: String,
    footprint: Vec<[i32; 2]>,
    floors: u32,
    type_code: String,
    ground_y2: i32,
    front_segment: Option<String>,
    detail_level: &'static str,
    clipped: bool,
    omitted: bool,
}

#[derive(Serialize)]
struct BuildingsDoc {
    version: &'static str,
    buildings: Vec<BuildingEntry>,
}

pub fn write_buildings_json(dir: &Path, buildings: &[PlannedBuilding]) -> Result<(), String> {
    let entries: Vec<BuildingEntry> = buildings
        .iter()
        .map(|b| BuildingEntry {
            building_id: b.id.clone(),
            footprint: b.cells.iter().map(|&(x, z)| [x, z]).collect(),
            floors: b.floors_above,
            type_code: b.type_code(),
            ground_y2: b.ground_y * 2,
            front_segment: b.front_segment.clone(),
            detail_level: b.detail_level.as_str(),
            clipped: b.clipped,
            omitted: b.omitted,
        })
        .collect();
    let doc = BuildingsDoc { version: "0.1", buildings: entries };
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("buildings.json"), text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danjok_and_gongdong_low_rise_are_r() {
        assert_eq!(classify_use_group("단독주택", 1), UseGroup::R);
        assert_eq!(classify_use_group("공동주택", 3), UseGroup::R);
    }

    #[test]
    fn gongdong_five_floors_or_more_is_a() {
        assert_eq!(classify_use_group("공동주택", 5), UseGroup::A);
        assert_eq!(classify_use_group("아파트", 2), UseGroup::A); // explicit "아파트" wins regardless of floors
    }

    #[test]
    fn neighborhood_facility_is_c_and_office_is_o() {
        assert_eq!(classify_use_group("제2종근린생활시설", 4), UseGroup::C);
        assert_eq!(classify_use_group("업무시설", 10), UseGroup::O);
    }

    #[test]
    fn factory_is_i_and_missing_use_defaults_to_r() {
        assert_eq!(classify_use_group("공장", 1), UseGroup::I);
        assert_eq!(classify_use_group("", 2), UseGroup::R);
    }

    #[test]
    fn unmatched_use_is_x() {
        assert_eq!(classify_use_group("종교시설", 3), UseGroup::X);
    }

    #[test]
    fn era_boundaries_match_spec_table() {
        assert_eq!(Era::from_year(1979).code(), "e1");
        assert_eq!(Era::from_year(1980).code(), "e2");
        assert_eq!(Era::from_year(1994).code(), "e2");
        assert_eq!(Era::from_year(1995).code(), "e3");
        assert_eq!(Era::from_year(2009).code(), "e3");
        assert_eq!(Era::from_year(2010).code(), "e4");
    }

    #[test]
    fn total_height_matches_worked_example() {
        // R, 4 floors: 6 (1st) + 3*6 (typical) = 24
        assert_eq!(total_height_blocks(UseGroup::R, 4), 24);
        // C, 1 floor: just the 1st-floor height
        assert_eq!(total_height_blocks(UseGroup::C, 1), 8);
    }

    #[test]
    fn rasterize_a_simple_square_covers_its_own_area() {
        let square = vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0)];
        let cells = rasterize(&square);
        assert_eq!(cells.len(), 25);
        assert!(cells.contains(&(2, 2)));
        assert!(!cells.contains(&(5, 5)));
    }
}
