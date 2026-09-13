//! SPEC_BuildingType.md §10 "경사지 건물", run before `facade::build` places
//! any wall (§10.7: "4·5번이 6번보다 앞이다"). `PlannedBuilding::ground_y` is
//! already §10.1's 전면 접도고 (nearest road point's solved height, set in
//! `compute_kr_buildings`); this module only does §10.3/§10.4's per-column
//! cut/fill against that fixed floor.
//!
//! **Disclosed simplifications**, both because this run's floor convention
//! makes the full rule moot and to keep this stage's scope to what
//! SPEC_Build.md M4 actually gates on ("건물이 지형에 뜨거나 묻히지 않는다"):
//! - §10.2's "7 이상: 단 나눔" (a two-tier retaining wall with a middle
//!   소단) is not modelled separately from the 3-6 case -- both get one
//!   single-tier fill/cut column. A real two-tier terrace is future work,
//!   not attempted here.
//! - §10.6 진입 계단 is not placed: this module defines 1층 바닥 as the
//!   nearest road *segment centerline*'s already-solved height (§10.1's own
//!   "확정된 도로 높이"), which sits within half a block of the sidewalk
//!   `kr_roads::sweep_and_place` actually draws -- under that convention the
//!   floor-to-road height gap the stair table keys off of is always in
//!   §10.6's own "1 (반블록): 없음, 문턱으로 둔다" row, so no case that would
//!   place a real staircase has been observed against this run's own
//!   output. Not implemented rather than guessed at with no gap to size it
//!   against.

use super::PlannedBuilding;
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;

/// SPEC_BuildingType.md §10.2's own cap: a single retaining tier taller than
/// one storey reads as absurd, so cut/fill height is capped here even though
/// the *real* Δ (already reported once per run by
/// `report_slope_resolution`) may run higher on 산복도로.
const MAX_SINGLE_TIER_BLOCKS: i32 = 6;

pub(super) fn grade_site(editor: &mut WorldEditor, b: &PlannedBuilding) {
    let front_y = b.ground_y;
    for (&(x, z), &terrain_h) in &b.terrain_y {
        let diff = terrain_h - front_y;
        if diff >= 3 {
            // §10.4 절토: clear room for the building up through the
            // original grade (plus margin for whatever canopy/decoration
            // sat on it), then mark the cut edge.
            for y in (front_y + 1)..=(terrain_h + 3) {
                editor.set_block_absolute(AIR, x, y, z, None, None);
            }
            editor.set_block_absolute(STONE_BRICKS, x, front_y, z, None, None);
        } else if diff > 0 {
            // §10.2 Δ≤2: flat enough to just clear down to the floor without a visible cut face.
            for y in (front_y + 1)..=terrain_h {
                editor.set_block_absolute(AIR, x, y, z, None, None);
            }
        } else if diff <= -3 {
            // §10.3 성토/축대: fill from original grade up to the floor,
            // capped at one storey's worth of retaining wall (see this
            // module's own doc for why the 7+ two-tier split isn't modelled).
            let fill_from = terrain_h.max(front_y - MAX_SINGLE_TIER_BLOCKS - 1);
            for y in fill_from..front_y {
                editor.set_block_absolute(retaining_material(x, z, y), x, y, z, None, None);
            }
            editor.set_block_absolute(STONE_BRICK_SLAB, x, front_y - 1, z, None, None);
        } else if diff < 0 {
            // §10.2 Δ≤2: plain fill, no distinct retaining-wall finish needed.
            for y in terrain_h..front_y {
                editor.set_block_absolute(retaining_material(x, z, y), x, y, z, None, None);
            }
        }
        // diff == 0: already at grade, nothing to do.
    }
}

/// §10.3: "cobblestone 기본, mossy_cobblestone 15% 혼입... 좌표 해시로 결정".
fn retaining_material(x: i32, z: i32, y: i32) -> Block {
    let h = super::geom::building_hash(&format!("{x}:{y}:{z}"));
    if h % 100 < 15 {
        MOSSY_COBBLESTONE
    } else {
        COBBLESTONE
    }
}
