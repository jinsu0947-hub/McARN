//! SPEC_Build.md M5 "가로 요소". SPEC_StreetFurniture.md §0: "전주와 전선이
//! 최우선이다" -- §2 (전주·전선) is the only section implemented so far;
//! the rest (§3 버스정류장, §4 가로등, SPEC_RoadSection.md §3 도색) land in
//! later passes, in the priority order §0 gives.
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
use crate::kr_roads::{self, Segment};
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
