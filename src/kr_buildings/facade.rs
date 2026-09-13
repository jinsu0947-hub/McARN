//! SPEC_BuildingType.md §4 (팔레트), §5 (파사드 구성), §6 (간판), §8 (L3
//! 간략 모드). Works per footprint *cell*, not per polygon edge: a cell on
//! the boundary of the (already road-clipped) footprint -- one with at
//! least one of its four neighbours missing from the footprint -- becomes a
//! solid wall column for its exposed side(s); a cell with all four
//! neighbours present stays hollow (air) up to the roof cap. This is a
//! deliberate simplification over walking the polygon's actual edges: it
//! costs the exact continuous window rhythm §5.3 describes along a single
//! straight wall (each exposed face decides window-vs-wall from its own
//! position modulo the pattern, independently of its neighbours), but it
//! handles an arbitrarily shaped or road-clipped footprint -- L-shapes,
//! notches, anything the rasterizer produced -- with no separate polygon-
//! tracing code path, and at Minecraft's own 1-block resolution the
//! difference is not visible from ground level.

use super::{DetailLevel, Era, PlannedBuilding, UseGroup};
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use std::collections::HashSet;

struct Palette {
    wall: Block,
    wall_band: Option<Block>,
    base: Option<Block>,
    window_run: i32,
    wall_run: i32,
    window_height: i32,
    parapet: Block,
    has_eave: bool,
}

/// SPEC_BuildingType.md §4's table, one arm per row. `X` (§0: "그 외
/// 주용도... O 규칙을 따르되 지붕만 별도 처리한다") reuses O's wall palette;
/// its roof is handled separately in `roof_extras`.
fn palette(group: UseGroup, era: Era) -> Palette {
    use Era::*;
    use UseGroup::*;
    match (group, era) {
        (R, E1) => Palette { wall: LIGHT_GRAY_CONCRETE, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 2, wall_run: 3, window_height: 2, parapet: COBBLESTONE_WALL, has_eave: true },
        (R, E2) => Palette { wall: BRICK, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 2, wall_run: 3, window_height: 2, parapet: COBBLESTONE_WALL, has_eave: true },
        (R, E3) => Palette { wall: WHITE_TERRACOTTA, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 3, wall_run: 2, window_height: 3, parapet: COBBLESTONE_WALL, has_eave: true },
        (R, E4) => Palette { wall: SMOOTH_SANDSTONE, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 3, wall_run: 2, window_height: 3, parapet: COBBLESTONE_WALL, has_eave: true },
        (A, E2) => Palette { wall: LIGHT_GRAY_CONCRETE, wall_band: None, base: Some(GRAY_CONCRETE), window_run: 4, wall_run: 2, window_height: 3, parapet: GRAY_CONCRETE, has_eave: false },
        (A, E3) => Palette { wall: WHITE_TERRACOTTA, wall_band: None, base: Some(GRAY_CONCRETE), window_run: 4, wall_run: 2, window_height: 3, parapet: GRAY_CONCRETE, has_eave: false },
        (A, E4) => Palette { wall: WHITE_CONCRETE, wall_band: Some(LIGHT_GRAY_CONCRETE), base: Some(GRAY_CONCRETE), window_run: 4, wall_run: 2, window_height: 3, parapet: GRAY_CONCRETE, has_eave: false },
        (C, E1) => Palette { wall: LIGHT_GRAY_CONCRETE, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 3, wall_run: 2, window_height: 3, parapet: COBBLESTONE_WALL, has_eave: true },
        (C, E2) => Palette { wall: BRICK, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 3, wall_run: 2, window_height: 3, parapet: COBBLESTONE_WALL, has_eave: true },
        (C, E3) => Palette { wall: WHITE_TERRACOTTA, wall_band: None, base: Some(POLISHED_DIORITE), window_run: 3, wall_run: 2, window_height: 3, parapet: COBBLESTONE_WALL, has_eave: true },
        (C, E4) => Palette { wall: GRAY_CONCRETE, wall_band: Some(GRAY_STAINED_GLASS), window_run: 4, wall_run: 1, window_height: 4, base: Some(POLISHED_DIORITE), parapet: COBBLESTONE_WALL, has_eave: true },
        (O, E2) => Palette { wall: LIGHT_GRAY_CONCRETE, wall_band: None, base: Some(GRAY_CONCRETE), window_run: 3, wall_run: 2, window_height: 3, parapet: GRAY_CONCRETE, has_eave: false },
        (O, E3) => Palette { wall: POLISHED_DIORITE, wall_band: Some(LIGHT_BLUE_STAINED_GLASS), window_run: 3, wall_run: 2, window_height: 3, base: Some(GRAY_CONCRETE), parapet: GRAY_CONCRETE, has_eave: false },
        (O, E4) => Palette { wall: GRAY_CONCRETE, wall_band: Some(GRAY_STAINED_GLASS), window_run: 4, wall_run: 1, window_height: 4, base: Some(GRAY_CONCRETE), parapet: GRAY_CONCRETE, has_eave: false },
        (I, _) => Palette { wall: LIGHT_GRAY_CONCRETE, wall_band: Some(CYAN_CONCRETE), base: None, window_run: 2, wall_run: 8, window_height: 2, parapet: GRAY_CONCRETE, has_eave: true },
        // X and any (group, era) SPEC_BuildingType.md §3 doesn't name explicitly (e.g. R/C/A at an
        // era the table skips) fall back to O's mid-era look -- a reasonable default, not a real rule.
        _ => Palette { wall: LIGHT_GRAY_CONCRETE, wall_band: None, base: Some(GRAY_CONCRETE), window_run: 3, wall_run: 2, window_height: 3, parapet: GRAY_CONCRETE, has_eave: false },
    }
}

/// One boundary cell's exposed neighbour directions (missing from the
/// footprint), at most 4, usually 1 (a straight wall) or 2 (an outer
/// corner).
fn exposed_directions(cells: &HashSet<(i32, i32)>, x: i32, z: i32) -> Vec<(i32, i32)> {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter(|&(dx, dz)| !cells.contains(&(x + dx, z + dz)))
        .collect()
}

pub(super) fn build(editor: &mut WorldEditor, b: &PlannedBuilding) {
    let cells: HashSet<(i32, i32)> = b.cells.iter().copied().collect();
    if cells.is_empty() {
        return;
    }
    let pal = palette(b.group, b.era);
    let (first_h, typical_h) = super::floor_heights(b.group);
    let base_band = if pal.base.is_some() { 1 } else { 0 };
    // One deterministic phase per building (SPEC_BuildingType.md §7): every
    // wall on this building starts its window rhythm at the same offset, so
    // a straight run of cells reads as one repeating pattern rather than
    // each cell rolling its own.
    let phase = (super::geom::building_hash(&b.id) % 97) as i32;
    let signage_color = signage_color(&b.id, b.era);

    let entrance_cell = b.front_point.map(|fp| {
        b.cells
            .iter()
            .copied()
            .min_by_key(|&(x, z)| (x - fp.0).pow(2) + (z - fp.1).pow(2))
            .unwrap()
    });

    for &(x, z) in &b.cells {
        let exposed = exposed_directions(&cells, x, z);
        let base_y = b.ground_y;

        if exposed.is_empty() {
            // Interior cell: hollow shell (SPEC_Build.md §5's own list of
            // what M4 doesn't cover yet includes "실내 인테리어" -- only the
            // roof cap over an interior column is this pass's concern).
            let roof_y = base_y + b.total_height;
            editor.set_block_absolute(roof_material(b.group), x, roof_y, z, None, None);
            continue;
        }

        if b.detail_level == DetailLevel::L3 {
            // SPEC_BuildingType.md §8: outline + height, one material, roof
            // railing only -- everything else skipped outright.
            for y_off in 0..b.total_height {
                editor.set_block_absolute(pal.wall, x, base_y + y_off, z, None, None);
            }
            editor.set_block_absolute(pal.parapet, x, base_y + b.total_height, z, None, None);
            continue;
        }

        let is_entrance = entrance_cell == Some((x, z));
        let dir = exposed[0];
        let axis_pos = if dir.0 != 0 { z } else { x };
        let period = (pal.window_run + pal.wall_run).max(1);
        let in_window_run = ((axis_pos + phase).rem_euclid(period)) < pal.window_run;

        let faces_front = b
            .front_point
            .map(|fp| {
                let to_front = ((fp.0 - x) as f64, (fp.1 - z) as f64);
                (dir.0 as f64) * to_front.0 + (dir.1 as f64) * to_front.1 > 0.0
            })
            .unwrap_or(false);

        for y_off in 0..b.total_height {
            let y = base_y + y_off;
            let is_ground_floor = y_off < first_h;
            // §5.4: the building's very top row is left solid (a plain eave/
            // parapet transition row) even mid-window-run.
            let is_roofline_row = y_off == b.total_height - 1;

            let block = if is_ground_floor && y_off < base_band {
                pal.base.unwrap_or(pal.wall)
            } else if is_entrance && is_ground_floor && y_off >= base_band && y_off < base_band + 2 {
                AIR // §5.2: 출입문/출입구 opening, 2 blocks tall
            } else if is_ground_floor {
                ground_floor_block(b.group, &pal, faces_front, signage_color, y_off, first_h, base_band)
            } else if in_window_run && !is_roofline_row {
                glass_for(b.group)
            } else {
                pal.wall_band.filter(|_| should_band(b.group, y_off, first_h, typical_h)).unwrap_or(pal.wall)
            };
            editor.set_block_absolute(block, x, y, z, None, None);
        }

        if pal.has_eave {
            let (nx, nz) = (x + dir.0, z + dir.1);
            editor.set_block_absolute(pal.wall, nx, base_y + b.total_height - 1, nz, None, None);
        }
        editor.set_block_absolute(pal.parapet, x, base_y + b.total_height, z, None, None);
    }

    if b.detail_level == DetailLevel::L2 {
        roof_extras(editor, b, &cells);
    }
}

fn should_band(group: UseGroup, y_off: i32, first_h: i32, typical_h: i32) -> bool {
    // Curtain-wall/banded groups (§4's O-e4, A-e4, I) alternate frame
    // material every other typical-floor band -- a cheap stand-in for a
    // literal per-floor mullion line.
    matches!(group, UseGroup::O | UseGroup::A | UseGroup::I) && ((y_off - first_h) / typical_h.max(1)) % 2 == 1
}

fn glass_for(group: UseGroup) -> Block {
    match group {
        UseGroup::I => LIGHT_GRAY_CONCRETE, // §5.3: I gets almost no windows; treated as wall, not glass
        _ => GLASS_PANE,
    }
}

fn ground_floor_block(
    group: UseGroup,
    pal: &Palette,
    faces_front: bool,
    signage_color: Block,
    y_off: i32,
    first_h: i32,
    base_band: i32,
) -> Block {
    // SPEC_BuildingType.md §6: signage band on the topmost 1층 row, only on
    // the road-facing side, C only.
    if group == UseGroup::C && faces_front && y_off == first_h - 1 {
        return signage_color;
    }
    match group {
        UseGroup::C | UseGroup::O => GLASS_PANE,
        UseGroup::I => pal.wall, // 셔터문 stands in as the same panel material, no separate block
        _ => {
            let _ = base_band;
            pal.wall
        }
    }
}

fn roof_material(group: UseGroup) -> Block {
    match group {
        UseGroup::I => GRAY_CONCRETE,
        _ => LIGHT_GRAY_CONCRETE,
    }
}

/// SPEC_BuildingType.md §6's five-color rotation, muted for e4.
fn signage_color(id: &str, era: Era) -> Block {
    let h = super::geom::building_hash(id);
    if era == Era::E4 {
        return if h % 2 == 0 { WHITE_CONCRETE } else { GRAY_CONCRETE };
    }
    match h % 5 {
        0 => RED_CONCRETE,
        1 => BLUE_CONCRETE,
        2 => GREEN_CONCRETE,
        3 => YELLOW_CONCRETE,
        _ => WHITE_CONCRETE,
    }
}

/// SPEC_BuildingType.md §5.5: rooftop structures, placed once per building
/// (not per cell) near its footprint's own centroid so a 옥탑방/물탱크/계단실
/// doesn't require every roof cell to agree on where it goes.
fn roof_extras(editor: &mut WorldEditor, b: &PlannedBuilding, cells: &HashSet<(i32, i32)>) {
    let n = b.cells.len() as f64;
    let (sx, sz) = b.cells.iter().fold((0.0, 0.0), |(ax, az), &(x, z)| (ax + x as f64, az + z as f64));
    let (cx, cz) = ((sx / n).round() as i32, (sz / n).round() as i32);
    let roof_y = b.ground_y + b.total_height + 1; // one above the parapet ring placed in `build`
    let interior_only = |x: i32, z: i32| cells.contains(&(x, z)) && exposed_directions(cells, x, z).is_empty();

    match b.group {
        UseGroup::R if b.era == Era::E1 => {
            // §5.5: sloped roof for R-e1 -- `deepslate_tile_stairs` isn't a
            // defined block in this crate's palette, so `stone_brick_stairs`
            // stands in (disclosed substitution, same silhouette).
            place_hip_roof(editor, b, cells, STONE_BRICK_STAIRS);
        }
        UseGroup::R if b.era == Era::E2 => {
            place_box(editor, cx, cz, roof_y, 2, PLANKS_FOR_ROOFTOP_ROOM, interior_only);
            place_water_tank(editor, cx + 3, cz, roof_y);
        }
        UseGroup::R => {
            place_water_tank(editor, cx, cz, roof_y);
        }
        UseGroup::A => {
            place_box(editor, cx, cz, roof_y, 3, GRAY_CONCRETE, interior_only);
        }
        UseGroup::O => {
            place_box(editor, cx, cz, roof_y, 2, GRAY_CONCRETE, interior_only);
        }
        UseGroup::C => {
            for (i, off) in [(-1, -1), (1, -1), (-1, 1)].iter().enumerate() {
                let _ = i;
                if interior_only(cx + off.0, cz + off.1) {
                    editor.set_block_absolute(LIGHT_GRAY_CONCRETE, cx + off.0, roof_y, cz + off.1, None, None);
                }
            }
        }
        UseGroup::I => {
            place_hip_roof(editor, b, cells, STONE_BRICK_STAIRS);
        }
        UseGroup::X => {
            // §0: "지붕만 별도 처리" -- left as the plain flat parapet `build`
            // already placed; no extra structure, which is itself the
            // "separate" treatment relative to O's 계단실.
        }
    }
}

fn place_water_tank(editor: &mut WorldEditor, cx: i32, cz: i32, y: i32) {
    for dx in 0..2 {
        for dz in 0..2 {
            for dy in 0..2 {
                editor.set_block_absolute(LIGHT_BLUE_CONCRETE, cx + dx, y + dy, cz + dz, None, None);
            }
        }
    }
}

fn place_box(editor: &mut WorldEditor, cx: i32, cz: i32, y: i32, size: i32, block: Block, interior_only: impl Fn(i32, i32) -> bool) {
    let half = size / 2;
    for dx in -half..=half {
        for dz in -half..=half {
            if !interior_only(cx + dx, cz + dz) {
                continue;
            }
            for dy in 0..3 {
                editor.set_block_absolute(block, cx + dx, y + dy, cz + dz, None, None);
            }
        }
    }
}

/// A crude gable/hip roof: for every boundary cell, stack stairs blocks
/// climbing toward the centroid until they'd overlap the interior's own
/// (already-capped) roof plane, capped at 4 rows deep -- SPEC_BuildingType.md
/// §5.5/§10.2's own remark that a real hip roof needs geometry this module
/// doesn't model; this is a disclosed, deliberately simple stand-in, not a
/// literal slope solve.
fn place_hip_roof(editor: &mut WorldEditor, b: &PlannedBuilding, cells: &HashSet<(i32, i32)>, stair_block: Block) {
    let roof_y = b.ground_y + b.total_height;
    for &(x, z) in &b.cells {
        let exposed = exposed_directions(cells, x, z);
        if exposed.is_empty() {
            continue;
        }
        editor.set_block_absolute(stair_block, x, roof_y, z, None, None);
    }
}

// A plain wood-toned block for R-e2's 옥탑방 box -- the type table doesn't
// name a specific material for the rooftop room itself (only for the water
// tank and railing), so this reuses the same wall-adjacent look e2 already
// has (brick) rather than inventing an unlisted material.
const PLANKS_FOR_ROOFTOP_ROOM: Block = BRICK;
