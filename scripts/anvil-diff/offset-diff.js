#!/usr/bin/env node
// One-off (2026-09-19 scope/bbox verification): diffs worldA's own bbox
// against worldB at an integer (dx,dz) offset, because the two worlds were
// generated with different origins (korea_planar_bbox shifted when the
// scope-union bbox expansion widened worldB's own bbox) -- anvil-diff.js
// itself assumes both worlds share one coordinate frame, which doesn't hold
// here. block_B = block_A + (dx, dz), same Y.
//
// Usage: node offset-diff.js <worldA> <worldB> <ax0> <az0> <ax1> <az1> <dx> <dz> [yMin] [yMax]

'use strict';
const fs = require('fs');
const path = require('path');

const [worldAArg, worldBArg, ax0, az0, ax1, az1, dxArg, dzArg, yMinArg, yMaxArg] = process.argv.slice(2);
const worldA = path.resolve(worldAArg);
const worldB = path.resolve(worldBArg);
const [xMin, zMin, xMax, zMax] = [ax0, az0, ax1, az1].map(Number);
const dx = Number(dxArg), dz = Number(dzArg);
const yMin = yMinArg ? Number(yMinArg) : -64;
const yMax = yMaxArg ? Number(yMaxArg) : 250;

function resolveRegionDir(worldPath) {
  const nested = path.join(worldPath, 'region');
  return fs.existsSync(nested) ? nested : worldPath;
}

async function readDataVersion(worldPath) {
  const nbt = require('prismarine-nbt');
  const raw = fs.readFileSync(path.join(worldPath, 'level.dat'));
  const { parsed } = await nbt.parse(raw);
  const root = parsed.value.Data ? parsed.value.Data.value : parsed.value;
  return root.DataVersion.value;
}

async function resolveMcVersion(worldPath) {
  const dv = await readDataVersion(worldPath);
  const mcData = require('minecraft-data');
  const versions = mcData.versionsByMinecraftVersion.pc;
  for (const v of Object.keys(versions)) if (versions[v].dataVersion === dv) return v;
  throw new Error(`DataVersion ${dv} unmapped`);
}

function regionFileExists(regionDir, cx, cz) {
  const rx = cx >> 5, rz = cz >> 5;
  return fs.existsSync(path.join(regionDir, `r.${rx}.${rz}.mca`));
}

function safeGetStateId(chunk, pos) {
  try { return chunk.getBlockStateId(pos); } catch { return 0; }
}
function blockName(Block, id) {
  try { return Block.fromStateId(id, 0).name; } catch { return `state#${id}`; }
}

async function main() {
  const mcVersion = await resolveMcVersion(worldA);
  console.log(`offset-diff: Minecraft ${mcVersion}`);
  console.log(`  A: ${worldA}  bbox x[${xMin},${xMax}] z[${zMin},${zMax}]`);
  console.log(`  B: ${worldB}  offset dx=${dx} dz=${dz}  y[${yMin},${yMax})`);

  const { Anvil } = require('prismarine-provider-anvil');
  const AnvilCtor = Anvil(mcVersion);
  const regionDirA = resolveRegionDir(worldA);
  const regionDirB = resolveRegionDir(worldB);
  const anvilA = new AnvilCtor(regionDirA);
  const anvilB = new AnvilCtor(regionDirB);
  const Block = require('prismarine-block')(mcVersion);

  const chunkCacheB = new Map();
  async function getChunkB(cx, cz) {
    const k = `${cx},${cz}`;
    if (chunkCacheB.has(k)) return chunkCacheB.get(k);
    const c = regionFileExists(regionDirB, cx, cz) ? await anvilB.load(cx, cz).catch(() => null) : null;
    chunkCacheB.set(k, c);
    if (chunkCacheB.size > 200) {
      const first = chunkCacheB.keys().next().value;
      chunkCacheB.delete(first);
    }
    return c;
  }

  const cxMin = xMin >> 4, cxMax = xMax >> 4;
  const czMin = zMin >> 4, czMax = zMax >> 4;

  let chunksCompared = 0, chunksWithDiff = 0, totalDiff = 0, aOnlyPresentButBAbsent = 0;
  const examples = [];

  for (let cx = cxMin; cx <= cxMax; cx++) {
    for (let cz = czMin; cz <= czMax; cz++) {
      if (!regionFileExists(regionDirA, cx, cz)) continue;
      const chunkA = await anvilA.load(cx, cz).catch(() => null);
      if (!chunkA) continue;
      chunksCompared++;

      const localXMin = Math.max(0, xMin - cx * 16);
      const localXMax = Math.min(15, xMax - cx * 16);
      const localZMin = Math.max(0, zMin - cz * 16);
      const localZMax = Math.min(15, zMax - cz * 16);

      let diffCount = 0;
      for (let y = yMin; y < yMax; y++) {
        for (let lx = localXMin; lx <= localXMax; lx++) {
          for (let lz = localZMin; lz <= localZMax; lz++) {
            const ax = cx * 16 + lx, az = cz * 16 + lz;
            const idA = safeGetStateId(chunkA, { x: lx, y, z: lz });
            const bx = ax + dx, bz = az + dz;
            const bcx = bx >> 4, bcz = bz >> 4;
            const chunkB = await getChunkB(bcx, bcz);
            const blx = ((bx % 16) + 16) % 16, blz = ((bz % 16) + 16) % 16;
            const idB = chunkB ? safeGetStateId(chunkB, { x: blx, y, z: blz }) : 0;
            if (idA !== idB) {
              diffCount++;
              if (examples.length < 20) {
                examples.push({ ax, y, az, a: blockName(Block, idA), b: blockName(Block, idB) });
              }
            }
          }
        }
      }
      if (diffCount > 0) {
        chunksWithDiff++;
        totalDiff += diffCount;
      }
    }
  }

  console.log('');
  console.log('offset-diff summary:');
  console.log(`  chunks compared (from A): ${chunksCompared}`);
  console.log(`  chunks with a difference: ${chunksWithDiff}`);
  console.log(`  total differing block positions: ${totalDiff}`);
  for (const e of examples) {
    console.log(`    (${e.ax},${e.y},${e.az}) A=${e.a} -> B(+offset)=${e.b}`);
  }
}

main().catch((e) => { console.error(e); process.exit(1); });
