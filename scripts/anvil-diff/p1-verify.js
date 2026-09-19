#!/usr/bin/env node
// One-off verification for the 2026-09-19 P1 regeneration session:
//   (a) no leftover vegetation on the road/sidewalk paved footprint
//       (SPEC_RoadSection.md §5.1), sampled from roadgraph.json's own
//       exported per-point road_half_width/curb_width/sidewalk_width;
//   (b) glass vs glass_pane usage on a sample of building facades
//       (SPEC_BuildingType.md §5.3), sampled from buildings.json's
//       type_code + footprint.
//
// Usage: node p1-verify.js <worldDir> <roadgraphJson> <buildingsJson>

'use strict';
const fs = require('fs');
const path = require('path');

const [worldDir, roadgraphPath, buildingsPath] = process.argv.slice(2);
if (!worldDir || !roadgraphPath || !buildingsPath) {
  console.error('Usage: node p1-verify.js <worldDir> <roadgraphJson> <buildingsJson>');
  process.exit(1);
}

const ROAD_PALETTE = new Set([
  'gray_concrete', 'light_gray_concrete', 'black_concrete', 'polished_andesite',
  'smooth_stone_slab', 'yellow_concrete', 'stone_bricks', 'white_concrete',
]);
function isVegetation(name) {
  return /_leaves$/.test(name) || /_log$/.test(name) || /_wood$/.test(name) ||
    /sapling$/.test(name) || name === 'vine' || name === 'moss_block';
}

async function readDataVersion(worldPath) {
  const levelDatPath = path.join(worldPath, 'level.dat');
  const nbt = require('prismarine-nbt');
  const raw = fs.readFileSync(levelDatPath);
  const { parsed } = await nbt.parse(raw);
  const root = parsed.value.Data ? parsed.value.Data.value : parsed.value;
  return root.DataVersion.value;
}

async function resolveMcVersion(worldPath) {
  const dv = await readDataVersion(worldPath);
  const mcData = require('minecraft-data');
  const versions = mcData.versionsByMinecraftVersion.pc;
  for (const v of Object.keys(versions)) {
    if (versions[v].dataVersion === dv) return v;
  }
  throw new Error(`DataVersion ${dv} unmapped`);
}

function chunkKey(cx, cz) { return cx + ',' + cz; }

async function main() {
  const mcVersion = await resolveMcVersion(worldDir);
  console.log('Minecraft version:', mcVersion);
  const { Anvil } = require('prismarine-provider-anvil');
  const AnvilCtor = Anvil(mcVersion);
  const regionDir = fs.existsSync(path.join(worldDir, 'region')) ? path.join(worldDir, 'region') : worldDir;
  const anvil = new AnvilCtor(regionDir);
  const Block = require('prismarine-block')(mcVersion);

  function regionExists(cx, cz) {
    const rx = cx >> 5, rz = cz >> 5;
    return fs.existsSync(path.join(regionDir, `r.${rx}.${rz}.mca`));
  }

  const chunkCache = new Map();
  async function getChunk(cx, cz) {
    const k = chunkKey(cx, cz);
    if (chunkCache.has(k)) return chunkCache.get(k);
    let c = null;
    if (regionExists(cx, cz)) {
      try { c = await anvil.load(cx, cz); } catch { c = null; }
    }
    chunkCache.set(k, c);
    if (chunkCache.size > 4000) {
      // crude LRU-ish cap: drop oldest inserted half
      const keys = [...chunkCache.keys()].slice(0, 2000);
      for (const kk of keys) chunkCache.delete(kk);
    }
    return c;
  }

  function blockNameAt(chunk, x, y, z) {
    if (!chunk) return 'air';
    const cx = Math.floor(x / 16), cz = Math.floor(z / 16);
    const lx = ((x % 16) + 16) % 16, lz = ((z % 16) + 16) % 16;
    try {
      const id = chunk.getBlockStateId({ x: lx, y, z: lz });
      return Block.fromStateId(id, 0).name;
    } catch {
      return 'air';
    }
  }

  // ---------- (a) vegetation-on-road check ----------
  console.log('\n=== (a) vegetation-on-road/sidewalk check ===');
  const rg = JSON.parse(fs.readFileSync(roadgraphPath, 'utf8'));
  const STRIDE = 8; // sample every 8th point per segment
  const candidates = new Map(); // "x,z" -> y2
  for (const seg of rg.segments) {
    const pts = seg.points;
    for (let i = 0; i < pts.length; i += STRIDE) {
      const p = pts[i];
      const prev = pts[i - 1] || pts[i + 1] || p;
      const next = pts[i + 1] || pts[i - 1] || p;
      const dx = next.pos[0] - prev.pos[0];
      const dz = next.pos[1] - prev.pos[1];
      const len = Math.hypot(dx, dz) || 1;
      const perp = [-dz / len, dx / len];
      const leftTotal = p.road_half_width[0] + p.curb_width[0] + p.sidewalk_width[0];
      const rightTotal = p.road_half_width[1] + p.curb_width[1] + p.sidewalk_width[1];
      for (let off = -leftTotal; off <= rightTotal; off++) {
        const px = p.pos[0] + Math.round(perp[0] * off);
        const pz = p.pos[1] + Math.round(perp[1] * off);
        candidates.set(px + ',' + pz, p.y2);
      }
    }
  }
  console.log(`sampled ${candidates.size} candidate paved columns (stride ${STRIDE})`);

  let checked = 0;
  let violations = [];
  let surfaceMismatch = 0;
  for (const [key, y2] of candidates) {
    const [x, z] = key.split(',').map(Number);
    const cx = x >> 4, cz = z >> 4;
    const chunk = await getChunk(cx, cz);
    if (!chunk) continue;
    const yFull = Math.floor(y2 / 2);
    let surfaceY = null;
    let surfaceName = null;
    for (let y = yFull + 3; y >= yFull - 2; y--) {
      const n = blockNameAt(chunk, x, y, z);
      if (n !== 'air') { surfaceY = y; surfaceName = n; break; }
    }
    checked++;
    if (surfaceY === null) continue;
    if (!ROAD_PALETTE.has(surfaceName)) {
      surfaceMismatch++;
      continue; // not clearly a paved surface at this sampled offset (e.g. taper edge rounding); skip
    }
    const above = blockNameAt(chunk, x, surfaceY + 1, z);
    if (isVegetation(above)) {
      violations.push({ x, y: surfaceY, z, surface: surfaceName, above });
    }
  }
  console.log(`checked ${checked} columns with a resolvable surface (${surfaceMismatch} didn't land on a recognized paved block, skipped)`);
  console.log(`vegetation-on-paving violations: ${violations.length}`);
  for (const v of violations.slice(0, 20)) {
    console.log(`  (${v.x},${v.y},${v.z}) ${v.surface} -> ${v.above} above it`);
  }

  // ---------- (b) glass vs glass_pane check ----------
  console.log('\n=== (b) building window material check ===');
  const bd = JSON.parse(fs.readFileSync(buildingsPath, 'utf8'));
  const bandTypes = new Set(['C-e4', 'O-e4']); // should stay glass_pane
  const noGlassTypes = new Set(['I-all', 'I']);
  let sampleBuildings = bd.buildings.filter((b) => !b.omitted && b.footprint && b.footprint.length > 0);
  // stride sample across the list for geographic spread
  const BSTRIDE = Math.max(1, Math.floor(sampleBuildings.length / 400));
  const picked = [];
  for (let i = 0; i < sampleBuildings.length; i += BSTRIDE) picked.push(sampleBuildings[i]);
  console.log(`sampling ${picked.length} of ${sampleBuildings.length} placed buildings`);

  const tally = {}; // type_code -> {glass, glass_pane}
  for (const b of picked) {
    const type = b.type_code || 'unknown';
    if (!tally[type]) tally[type] = { glass: 0, glass_pane: 0 };
    const groundY = Math.floor((b.ground_y2 || 0) / 2);
    // sample a handful of footprint cells (likely mix of exposed/interior; fine for an aggregate signal)
    const cells = b.footprint.slice(0, 6);
    for (const [x, z] of cells) {
      const cx = x >> 4, cz = z >> 4;
      const chunk = await getChunk(cx, cz);
      if (!chunk) continue;
      for (let dy = 3; dy <= 26; dy++) {
        const n = blockNameAt(chunk, x, groundY + dy, z);
        if (n === 'glass') tally[type].glass++;
        else if (n === 'glass_pane') tally[type].glass_pane++;
      }
    }
  }
  const rows = Object.entries(tally).sort((a, b2) => (b2[1].glass + b2[1].glass_pane) - (a[1].glass + a[1].glass_pane));
  console.log('type_code       glass  glass_pane  expected');
  for (const [type, counts] of rows) {
    const expectPane = bandTypes.has(type);
    const expectNone = noGlassTypes.has(type);
    const expected = expectNone ? 'no glass' : expectPane ? 'glass_pane (band)' : 'glass';
    console.log(`${type.padEnd(15)} ${String(counts.glass).padStart(5)}  ${String(counts.glass_pane).padStart(10)}  ${expected}`);
  }

  console.log('\ndone.');
}

main().catch((e) => { console.error(e); process.exit(1); });
