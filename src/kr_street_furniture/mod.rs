//! SPEC_Build.md M5 "가로 요소". SPEC_StreetFurniture.md §0: "전주와 전선이
//! 최우선이다" -- §2 (전주·전선), §3 (버스정류장), and SPEC_RoadSection.md §3
//! (교차로 횡단보도·정지선) are implemented; §4 가로등 and §3's 차선 점선
//! (3차로 이상 도로에만 적용되는 백색 점선 -- 실제 `lanes` 데이터가 있는
//! 세그먼트가 적어 뒤로 미뤘다, 명시적 제외이지 누락이 아니다) land in a
//! later pass.
//!
//! Placement column: every element in this module lives in the single curb-
//! side sidewalk cell `kr_roads::furniture_column` resolves per road grade
//! (SPEC_StreetFurniture.md §1). Collisions between different kinds of
//! furniture sharing that one column are resolved by §8's fixed priority
//! (버스정류장 > 신호등 > 전주 > 도로표지 > 가로등 > 가로수 > 볼라드·기타) --
//! `ClaimedColumns` is the shared registry later passes (bus stops first,
//! per that ordering) check before claiming a cell for their own use.

use crate::block_definitions::*;
use crate::kr_buildings::{PlannedBuilding, UseGroup};
use crate::kr_roads::{self, RoadClass, Segment};
use crate::kr_transit::StopEntry;
use crate::world_editor::WorldEditor;
use fnv::FnvHashMap;
use std::collections::HashSet;

/// SPEC_StreetFurniture.md §8's shared placement-column registry: every
/// module in priority order inserts the columns it claims, and checks
/// existing entries before claiming a column another (higher-priority)
/// pass already took. Keyed by absolute (x, z); value is a short tag for
/// debugging ("pole", "stop", ...), not read by placement logic itself.
pub type ClaimedColumns = FnvHashMap<(i32, i32), &'static str>;

/// SPEC_StreetFurniture.md §2.1.
const POLE_SPACING: i32 = 60;
const POLE_HEIGHT: i32 = 18;
/// Blocks below the pole's own top, one per cross-arm tier ("2~3단").
const CROSS_ARM_TIERS: [i32; 2] = [1, 4];
/// "전주 3개마다 1개".
const TRANSFORMER_EVERY: u32 = 3;
/// §2.3.
const SERVICE_DROP_RADIUS: i32 = 12;

struct PlacedPole {
    x: i32,
    z: i32,
    /// Absolute Y of each cross-arm tier, same order as `CROSS_ARM_TIERS`.
    tier_y: [i32; 2],
}

/// Places every 전주/전선/인입선 SPEC_StreetFurniture.md §2 describes, for
/// every segment whose grade has a defined furniture column (§1 -- A and F
/// are skipped, see `kr_roads::furniture_column`'s own doc).
///
/// `claimed` is SPEC_StreetFurniture.md §8's shared registry: called after
/// every higher-priority pass (bus stops, once implemented) has already
/// claimed its columns, so a pole whose spacing would land on an occupied
/// column is skipped there -- "밀려난 요소 중 간격 규칙이 있는 것은 다음
/// 주기 위치에서 재개한다" (§8): the spacing counter is never reset, so a
/// skipped pole just leaves a wider-than-`POLE_SPACING` gap once.
pub fn place_utility_lines<'a>(
    editor: &mut WorldEditor,
    segments: impl IntoIterator<Item = &'a Segment>,
    buildings: &[PlannedBuilding],
    claimed: &mut ClaimedColumns,
) {
    let service_drop_index = index_service_drop_buildings(buildings);
    for seg in segments {
        let Some((left_off, right_off)) = kr_roads::furniture_column(seg.class()) else {
            continue;
        };
        let points = seg.points();
        if points.len() < 2 {
            continue;
        }
        // Fixed per segment, per §2.1's own "좌우는 구간 단위로 고정한다" --
        // hashed off the segment's own start point, not the (shared, ambient)
        // per-column coordinate hash, so two segments starting at the same
        // point can't both land on the same side by construction alone.
        let (sx, sz) = points[0].xz();
        let offset = if kr_roads::coord_hash(sx, sz) % 2 == 0 { left_off } else { right_off };

        let mut poles: Vec<PlacedPole> = Vec::new();
        let mut since_last = 0.0_f64; // distance walked since the last pole, carried across point pairs
        for w in points.windows(2) {
            let (x0, z0) = w[0].xz();
            let (x1, z1) = w[1].xz();
            let seg_len = (((x1 - x0).pow(2) + (z1 - z0).pow(2)) as f64).sqrt();
            if seg_len < 1e-6 {
                continue;
            }
            let dir = ((x1 - x0) as f64 / seg_len, (z1 - z0) as f64 / seg_len);
            let perp = (-dir.1, dir.0);

            let mut walked = 0.0_f64; // distance walked within this point pair
            while since_last + (seg_len - walked) >= POLE_SPACING as f64 {
                walked += POLE_SPACING as f64 - since_last;
                since_last = 0.0;
                let t = walked / seg_len;
                let px = x0 + ((x1 - x0) as f64 * t).round() as i32 + (perp.0 * offset as f64).round() as i32;
                let pz = z0 + ((z1 - z0) as f64 * t).round() as i32 + (perp.1 * offset as f64).round() as i32;
                if let Some(pole) = try_place_pole(editor, px, pz, poles.len() as u32, claimed) {
                    poles.push(pole);
                }
            }
            since_last += seg_len - walked;
        }

        // §2.2: one wire per tier between every pair of adjacent poles.
        for pair in poles.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            for (tier_idx, _) in CROSS_ARM_TIERS.iter().enumerate() {
                place_sagging_wire(editor, (a.x, a.tier_y[tier_idx], a.z), (b.x, b.tier_y[tier_idx], b.z));
            }
        }

        // §2.3: 인입선, one draw per pole (not per tier -- a single service
        // drop per pole reads as one line to the building, not a bundle).
        for pole in &poles {
            maybe_place_service_drop(editor, pole, &service_drop_index, buildings);
        }
    }
}

/// Attempts to claim `(x, z)` for a pole; `None` if SPEC_StreetFurniture.md
/// §8 already gave that column to something else. On success, places the
/// pole shaft, its cross-arms, and (every `TRANSFORMER_EVERY`th pole) its
/// transformer, and returns the placed pole's tier heights for wiring.
fn try_place_pole(editor: &mut WorldEditor, x: i32, z: i32, index: u32, claimed: &mut ClaimedColumns) -> Option<PlacedPole> {
    if claimed.contains_key(&(x, z)) {
        return None;
    }
    claimed.insert((x, z), "pole");

    let base = editor.get_ground_level(x, z);
    for dy in 1..=POLE_HEIGHT {
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base + dy, z, None, Some(&[]));
    }

    let top = base + POLE_HEIGHT;
    let mut tier_y = [0; 2];
    for (i, &below_top) in CROSS_ARM_TIERS.iter().enumerate() {
        let y = top - below_top;
        tier_y[i] = y;
        editor.set_block_absolute(POLISHED_ANDESITE_SLAB, x + 1, y, z, None, Some(&[]));
        editor.set_block_absolute(POLISHED_ANDESITE_SLAB, x - 1, y, z, None, Some(&[]));
    }

    if index % TRANSFORMER_EVERY == 0 {
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, top - 4, z, None, Some(&[]));
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, top - 5, z, None, Some(&[]));
    }

    Some(PlacedPole { x, z, tier_y })
}

/// SPEC_StreetFurniture.md §2.2: straight line between the two tier
/// attachment points, sagging exactly 1 block at the midpoint ("중간을
/// 1블록 내려 처지게 한다") -- a fixed 1-block sag, not `element_processing/
/// power.rs`'s distance-scaled `max_sag` (that OSM path draws lines of very
/// different real-world voltage/span; every KR pole span here is the same
/// fixed `POLE_SPACING`, so a fixed sag is the right read of "1블록", not an
/// approximation of it).
fn place_sagging_wire(editor: &mut WorldEditor, a: (i32, i32, i32), b: (i32, i32, i32)) {
    let axis_x = (b.0 - a.0).abs() >= (b.2 - a.2).abs();
    let wire_block = if axis_x { CHAIN_X } else { CHAIN_Z };
    let line = crate::bresenham::bresenham_line(a.0, 0, a.2, b.0, 0, b.2);
    let denom = (line.len().saturating_sub(1)).max(1) as f64;
    for (idx, &(lx, _, lz)) in line.iter().enumerate() {
        let t = idx as f64 / denom;
        let sag = (4.0 * t * (1.0 - t)).round() as i32; // peaks at 1 block, t=0.5
        let line_y = (a.1 as f64 + (b.1 - a.1) as f64 * t).round() as i32;
        editor.set_block_absolute(wire_block, lx, line_y - sag, lz, None, Some(&[]));
    }
}

/// SPEC_StreetFurniture.md §2.3: nearest C/R building within
/// `SERVICE_DROP_RADIUS`, drawn 50% of the time (coordinate-hashed on the
/// pole's own position, so the same run always draws the same poles).
fn maybe_place_service_drop(
    editor: &mut WorldEditor,
    pole: &PlacedPole,
    index: &FnvHashMap<(i32, i32), usize>,
    buildings: &[PlannedBuilding],
) {
    let Some((bx, bz, roof_y, _building_idx)) = nearest_service_drop_building(pole.x, pole.z, index, buildings) else {
        return;
    };
    if kr_roads::coord_hash(pole.x, pole.z) % 2 != 0 {
        return;
    }
    // Attach from the lower cross-arm tier (index 1: closer to the street-
    // facing side of the pole, the natural drop-off point) to the roof.
    let start = (pole.x, pole.tier_y[1], pole.z);
    let end = (bx, roof_y, bz);
    let axis_x = (end.0 - start.0).abs() >= (end.2 - start.2).abs();
    let wire_block = if axis_x { CHAIN_X } else { CHAIN_Z };
    for &(lx, ly, lz) in &crate::bresenham::bresenham_line(start.0, start.1, start.2, end.0, end.1, end.2) {
        editor.set_block_absolute(wire_block, lx, ly, lz, None, Some(&[]));
    }
}

/// Every C/R building's footprint cell -> that building's index, so a pole
/// can find "is there a C/R building within `SERVICE_DROP_RADIUS`" by
/// scanning a small window instead of every building in the run.
fn index_service_drop_buildings(buildings: &[PlannedBuilding]) -> FnvHashMap<(i32, i32), usize> {
    let mut index = FnvHashMap::default();
    for (i, b) in buildings.iter().enumerate() {
        let Some((cells, _roof_y, group, _id)) = b.service_drop_info() else { continue };
        if !matches!(group, UseGroup::C | UseGroup::R) {
            continue;
        }
        for &(x, z) in cells {
            index.insert((x, z), i);
        }
    }
    index
}

fn nearest_service_drop_building<'a>(
    px: i32,
    pz: i32,
    index: &FnvHashMap<(i32, i32), usize>,
    buildings: &'a [PlannedBuilding],
) -> Option<(i32, i32, i32, usize)> {
    let mut best: Option<(i32, i32, i32, i32, usize)> = None; // (dist2, x, z, roof_y, idx)
    let mut seen: HashSet<usize> = HashSet::new();
    for dx in -SERVICE_DROP_RADIUS..=SERVICE_DROP_RADIUS {
        for dz in -SERVICE_DROP_RADIUS..=SERVICE_DROP_RADIUS {
            let cell = (px + dx, pz + dz);
            let Some(&idx) = index.get(&cell) else { continue };
            if !seen.insert(idx) {
                continue;
            }
            let dist2 = dx * dx + dz * dz;
            if dist2 > SERVICE_DROP_RADIUS * SERVICE_DROP_RADIUS {
                continue;
            }
            let Some((_, roof_y, _, _)) = buildings[idx].service_drop_info() else { continue };
            if best.is_none_or(|(bd, ..)| dist2 < bd) {
                best = Some((dist2, cell.0, cell.1, roof_y, idx));
            }
        }
    }
    best.map(|(_, x, z, roof_y, idx)| (x, z, roof_y, idx))
}

// ---------------------------------------------------------------------
// SPEC_StreetFurniture.md §3 -- 버스정류장
// ---------------------------------------------------------------------

const SHELTER_HALF_LENGTH: i32 = 3; // "폭 7블록", 3 either side of the stop's own point
const SHELTER_DEPTH: i32 = 3; // "깊이 3블록", into the sidewalk from the curb
const SHELTER_HEIGHT: i32 = 5;
const SIGN_HEIGHT: i32 = 5;
/// A stop more than this far from the nearest road point is almost
/// certainly a data mismatch (wrong route, or a stop this run's bbox clips
/// mid-approach) -- SPEC_StreetFurniture.md gives no explicit cutoff, but
/// placing a shelter tens of blocks from any road would misread as a
/// placement bug, not a legitimate stop. `kr_buildings`' own `nearest_front`
/// has no such cutoff because a building's footprint always has *a* nearest
/// road by construction; a stop's raw coordinate doesn't have that guarantee.
const MAX_STOP_TO_ROAD_DIST: i32 = 40;

/// One reference point along the road network: position, the grade at that
/// point, and the local forward direction (from this point to the next).
struct RoadRef {
    x: i32,
    z: i32,
    class: RoadClass,
    dir: (f64, f64),
}

/// Places every stop in `stops` (SPEC_Build.md M2's `stops.json` positions)
/// per SPEC_StreetFurniture.md §3: a shelter for B/C/D grades with room for
/// one (§3.1), a stand-alone sign otherwise (§3.2), and a road-paint edge
/// line the stop's own length (§3.3).
///
/// Real coordinates, not `furniture_column`'s generic per-grade column,
/// decide *where along the road* each stop sits (§3's own "정류소 좌표
/// 데이터의 각 지점에 배치한다") -- only which *side* and how far into the
/// sidewalk reuse that resolution, the same way this module's other
/// elements do.
///
/// Must run before `place_utility_lines` and claim into the same
/// `claimed`: SPEC_StreetFurniture.md §8 puts bus stops first ("정류장이
/// 최우선인 이유 -- 노선 주행 재현이 목적이므로 정류소는 위치가 정확해야
/// 한다").
pub fn place_bus_stops<'a>(
    editor: &mut WorldEditor,
    stops: &[StopEntry],
    segments: impl IntoIterator<Item = &'a Segment>,
    claimed: &mut ClaimedColumns,
) {
    if stops.is_empty() {
        return;
    }
    let refs = collect_road_refs(segments);
    if refs.is_empty() {
        return;
    }
    for stop in stops {
        let (sx, sz) = stop.pos();
        let Some(r) = nearest_road_ref(sx, sz, &refs) else { continue };
        if (r.x - sx).pow(2) + (r.z - sz).pow(2) > MAX_STOP_TO_ROAD_DIST * MAX_STOP_TO_ROAD_DIST {
            continue;
        }
        let Some((left_off, right_off)) = kr_roads::furniture_column(r.class) else { continue };
        let perp = (-r.dir.1, r.dir.0);
        let side_point = |offset: i32| {
            (r.x + (perp.0 * offset as f64).round() as i32, r.z + (perp.1 * offset as f64).round() as i32)
        };
        let (left_x, left_z) = side_point(left_off);
        let (right_x, right_z) = side_point(right_off);
        let left_dist2 = (left_x - sx).pow(2) + (left_z - sz).pow(2);
        let right_dist2 = (right_x - sx).pow(2) + (right_z - sz).pow(2);
        let (anchor_x, anchor_z, side_sign) =
            if left_dist2 <= right_dist2 { (left_x, left_z, -1.0) } else { (right_x, right_z, 1.0) };
        let into_sidewalk = (perp.0 * side_sign, perp.1 * side_sign); // toward the anchor's own side, deeper into the sidewalk

        let spec_allows_shelter = matches!(r.class, RoadClass::B | RoadClass::C | RoadClass::D);
        let sidewalk_wide_enough = kr_roads::sidewalk_width(r.class) >= 3;
        let has_shelter = spec_allows_shelter && sidewalk_wide_enough;

        let cell = |along: i32, depth: i32| {
            (
                anchor_x + (r.dir.0 * along as f64).round() as i32 + (into_sidewalk.0 * depth as f64).round() as i32,
                anchor_z + (r.dir.1 * along as f64).round() as i32 + (into_sidewalk.1 * depth as f64).round() as i32,
            )
        };

        if has_shelter {
            place_shelter(editor, cell, claimed);
            place_sign(editor, cell(SHELTER_HALF_LENGTH, 0), claimed);
        } else {
            place_sign(editor, (anchor_x, anchor_z), claimed);
        }

        place_stop_line(editor, r, side_sign);
    }
}

fn collect_road_refs<'a>(segments: impl IntoIterator<Item = &'a Segment>) -> Vec<RoadRef> {
    let mut refs = Vec::new();
    for seg in segments {
        let points = seg.points();
        if points.len() < 2 {
            continue;
        }
        for w in points.windows(2) {
            let (x0, z0) = w[0].xz();
            let (x1, z1) = w[1].xz();
            let len = (((x1 - x0).pow(2) + (z1 - z0).pow(2)) as f64).sqrt();
            if len < 1e-6 {
                continue;
            }
            refs.push(RoadRef { x: x0, z: z0, class: seg.class(), dir: ((x1 - x0) as f64 / len, (z1 - z0) as f64 / len) });
        }
        if let (Some(last), Some(prev)) = (points.last(), points.get(points.len().wrapping_sub(2))) {
            let (x0, z0) = prev.xz();
            let (x1, z1) = last.xz();
            let len = (((x1 - x0).pow(2) + (z1 - z0).pow(2)) as f64).sqrt().max(1e-6);
            refs.push(RoadRef { x: x1, z: z1, class: seg.class(), dir: ((x1 - x0) as f64 / len, (z1 - z0) as f64 / len) });
        }
    }
    refs
}

fn nearest_road_ref<'a>(x: i32, z: i32, refs: &'a [RoadRef]) -> Option<&'a RoadRef> {
    refs.iter().min_by_key(|r| (r.x - x).pow(2) + (r.z - z).pow(2))
}

fn place_shelter(editor: &mut WorldEditor, cell: impl Fn(i32, i32) -> (i32, i32), claimed: &mut ClaimedColumns) {
    let (bx, bz) = cell(0, 0);
    let base = editor.get_ground_level(bx, bz);
    let is_corner = |a: i32, d: i32| (a == -SHELTER_HALF_LENGTH || a == SHELTER_HALF_LENGTH) && (d == 0 || d == SHELTER_DEPTH - 1);
    let is_end = |a: i32| a == -SHELTER_HALF_LENGTH || a == SHELTER_HALF_LENGTH;
    let is_back = |d: i32| d == SHELTER_DEPTH - 1;
    for a in -SHELTER_HALF_LENGTH..=SHELTER_HALF_LENGTH {
        for d in 0..SHELTER_DEPTH {
            let (x, z) = cell(a, d);
            claimed.entry((x, z)).or_insert("stop");
            if is_corner(a, d) {
                for dy in 1..=SHELTER_HEIGHT {
                    editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base + dy, z, None, Some(&[]));
                }
            } else if is_end(a) || is_back(d) {
                for dy in 1..SHELTER_HEIGHT {
                    editor.set_block_absolute(GLASS_PANE, x, base + dy, z, None, Some(&[]));
                }
            }
            editor.set_block_absolute(SMOOTH_STONE_SLAB, x, base + SHELTER_HEIGHT, z, None, Some(&[]));
            if is_back(d) && !is_end(a) {
                editor.set_block_absolute(SMOOTH_STONE_SLAB, x, base + 1, z, None, Some(&[]));
            }
        }
    }
}

fn place_sign(editor: &mut WorldEditor, (x, z): (i32, i32), claimed: &mut ClaimedColumns) {
    if claimed.contains_key(&(x, z)) {
        return; // shelter (if any) already stands here -- same priority tier, first write wins
    }
    claimed.insert((x, z), "stop");
    let base = editor.get_ground_level(x, z);
    for dy in 1..=SIGN_HEIGHT {
        editor.set_block_absolute(IRON_BARS, x, base + dy, z, None, Some(&[]));
    }
    editor.set_block_absolute(YELLOW_CONCRETE, x, base + SIGN_HEIGHT + 1, z, None, Some(&[]));
}

/// §3.3: solid yellow line along the carriageway edge, the stop's own
/// length ("정류소 길이만큼"). Measured from `r`'s own centerline point --
/// unlike the shelter/sign, this doesn't need the stop's real offset from
/// it, just which side.
fn place_stop_line(editor: &mut WorldEditor, r: &RoadRef, side_sign: f64) {
    let Some((left_off, right_off)) = kr_roads::carriage_edge_column(r.class) else { return };
    let offset = if side_sign < 0.0 { left_off } else { right_off };
    let perp = (-r.dir.1, r.dir.0);
    let ex = r.x + (perp.0 * offset as f64).round() as i32;
    let ez = r.z + (perp.1 * offset as f64).round() as i32;
    let base = editor.get_ground_level(ex, ez);
    for a in -SHELTER_HALF_LENGTH..=SHELTER_HALF_LENGTH {
        let x = ex + (r.dir.0 * a as f64).round() as i32;
        let z = ez + (r.dir.1 * a as f64).round() as i32;
        editor.set_block_absolute(YELLOW_CONCRETE, x, base, z, None, Some(&[]));
    }
}

// ---------------------------------------------------------------------
// SPEC_RoadSection.md §3 -- 횡단보도·정지선
// ---------------------------------------------------------------------

/// "횡단보도 길이 6블록" (§3, ×1.75 표).
const CROSSWALK_LENGTH: i32 = 6;
/// Distance back from the intersection node the crosswalk's near edge
/// sits, clear of SPEC_RoadSection.md §4's own curb radius (8-20 blocks) --
/// a fixed, conservative setback rather than reading that radius back out
/// of the (not-yet-modelled here) intersection geometry.
const CROSSWALK_SETBACK: i32 = 10;

/// Places a crosswalk + stop line on every C-grade-or-above segment's
/// approach to a real intersection (a node 3+ segment-ends touch -- a
/// through-node where one road just continues doesn't count). §3's own
/// rule: "횡단보도: C등급 이상 교차로의 각 진입부. D 이하는 생략."
pub fn place_crosswalks<'a>(editor: &mut WorldEditor, segments: impl IntoIterator<Item = &'a Segment> + Clone) {
    let mut node_degree: FnvHashMap<&str, u32> = FnvHashMap::default();
    for seg in segments.clone() {
        let (f, t) = kr_roads::endpoints(seg);
        *node_degree.entry(f).or_insert(0) += 1;
        *node_degree.entry(t).or_insert(0) += 1;
    }

    for seg in segments {
        if !matches!(seg.class(), RoadClass::A | RoadClass::B | RoadClass::C) {
            continue;
        }
        let points = seg.points();
        if points.len() < 2 {
            continue;
        }
        let (f, t) = kr_roads::endpoints(seg);
        if node_degree.get(f).copied().unwrap_or(0) >= 3 {
            place_one_crosswalk(editor, seg.class(), points, false);
        }
        if node_degree.get(t).copied().unwrap_or(0) >= 3 {
            place_one_crosswalk(editor, seg.class(), points, true);
        }
    }
}

/// `from_end`: approach the intersection from the segment's *last* point
/// backward, instead of its first point forward -- the same points list
/// either way, just walked from the other side.
fn place_one_crosswalk(editor: &mut WorldEditor, class: RoadClass, points: &[kr_roads::ProfilePoint], from_end: bool) {
    let n = points.len();
    // Walk inward from the intersection end by CROSSWALK_SETBACK blocks
    // (points run roughly 1 block apart), then need CROSSWALK_LENGTH more
    // beyond that for the stripes themselves.
    let idx_at = |steps_in: usize| -> usize {
        if from_end { n - 1 - steps_in.min(n - 1) } else { steps_in.min(n - 1) }
    };
    let near_idx = idx_at(CROSSWALK_SETBACK as usize);
    let far_idx = idx_at((CROSSWALK_SETBACK + CROSSWALK_LENGTH) as usize);
    if near_idx == far_idx {
        return; // segment too short for both a setback and a crosswalk
    }
    let (nx, nz) = points[near_idx].xz();
    let (fx, fz) = points[far_idx].xz();
    let dx = (fx - nx) as f64;
    let dz = (fz - nz) as f64;
    let len = (dx * dx + dz * dz).sqrt();
    if len < 1.0 {
        return;
    }
    let dir = (dx / len, dz / len);
    let perp = (-dir.1, dir.0);
    let half_w = kr_roads::carriageway_half_width(class);
    let base = editor.get_ground_level(nx, nz);

    // Stop line: one solid row, right at the crosswalk's near edge (the
    // side closer to the intersection the vehicle is stopping for).
    for w in -half_w..=half_w {
        let x = nx + (perp.0 * w as f64).round() as i32;
        let z = nz + (perp.1 * w as f64).round() as i32;
        editor.set_block_absolute(WHITE_CONCRETE, x, base, z, None, Some(&[]));
    }

    // Crosswalk: alternating 1-block stripes across CROSSWALK_LENGTH,
    // starting one block past the stop line so the two don't merge into
    // one wide band.
    for step in 1..=CROSSWALK_LENGTH {
        let cx = nx + (dir.0 * step as f64).round() as i32;
        let cz = nz + (dir.1 * step as f64).round() as i32;
        let stripe = step % 2 == 1;
        for w in -half_w..=half_w {
            let x = cx + (perp.0 * w as f64).round() as i32;
            let z = cz + (perp.1 * w as f64).round() as i32;
            let block = if stripe { WHITE_CONCRETE } else { GRAY_CONCRETE };
            editor.set_block_absolute(block, x, base, z, None, Some(&[]));
        }
    }
}
