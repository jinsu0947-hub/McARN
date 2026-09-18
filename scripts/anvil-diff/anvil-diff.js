#!/usr/bin/env node
// Reusable Anvil-world block diff, chunk-granularity summary.
//
// PROGRESS.md §5 used to say Anvil-level world verification was "그때그때
// 짜서 했다" -- a one-off script rebuilt from scratch each time it was
// needed. This is that script, kept, so the next verification (starting
// with the scope-generation-range execution check in §6) doesn't repeat
// the cost of writing it again.
//
// Usage:
//   node anvil-diff.js <worldA> <worldB> --bbox xMin,zMin,xMax,zMax [options]
//
// <worldA>/<worldB> are Java Anvil world folders (containing region/*.mca,
// or the region files directly -- both layouts are probed, see
// `resolveRegionDir`). Only chunks whose 16x16 column overlaps `--bbox`
// (block coordinates, world space) are compared.
//
// Options:
//   --y-min <int>       Lowest Y to compare (default: -64)
//   --y-max <int>       Highest Y to compare, exclusive (default: 320)
//   --mc-version <ver>  Force a minecraft-data version string instead of
//                       reading it from worldA's level.dat DataVersion
//   --top <n>           How many representative differing chunks to print
//                       in detail (default: 20)
//   --json <path>       Also write the full summary as JSON to this path
//
// Output: total chunks compared, chunks with any difference, total
// differing block positions, and (for up to --top chunks) a representative
// differing coordinate plus the two block names there -- not a full
// per-block listing (SPEC_Validation's own reasoning applies here too:
// a count and a place to look beats a wall of coordinates).
//
// Read-only by design: existence of each region file is checked with
// `fs.existsSync` *before* touching prismarine-provider-anvil, because
// `Anvil.getRegion` creates a region file if one is missing (this module's
// own `RegionFile._initialize`, `fs.open(path, 'w+')` on ENOENT) -- calling
// it on a region that doesn't exist yet would litter the world folder with
// empty .mca files. A missing region is treated as "every chunk in it is
// absent" without ever creating one.

'use strict';

const fs = require('fs');
const path = require('path');

function parseArgs(argv) {
  const args = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--bbox') args.bbox = argv[++i];
    else if (a === '--y-min') args.yMin = parseInt(argv[++i], 10);
    else if (a === '--y-max') args.yMax = parseInt(argv[++i], 10);
    else if (a === '--mc-version') args.mcVersion = argv[++i];
    else if (a === '--top') args.top = parseInt(argv[++i], 10);
    else if (a === '--json') args.json = argv[++i];
    else args._.push(a);
  }
  return args;
}

function usageAndExit(msg) {
  if (msg) console.error('Error: ' + msg + '\n');
  console.error(
    'Usage: node anvil-diff.js <worldA> <worldB> --bbox xMin,zMin,xMax,zMax ' +
      '[--y-min N] [--y-max N] [--mc-version 1.21.1] [--top N] [--json path]'
  );
  process.exit(msg ? 1 : 0);
}

// Java worlds Arnis writes put region files directly under
// `<world>/region/`. Some ad hoc test outputs (this script's own earlier
// incarnations) pointed straight at a region directory instead -- probe
// both so a caller doesn't have to remember which shape a given path is.
function resolveRegionDir(worldPath) {
  const nested = path.join(worldPath, 'region');
  if (fs.existsSync(nested) && fs.statSync(nested).isDirectory()) return nested;
  return worldPath;
}

async function readDataVersion(worldPath) {
  const levelDatPath = path.join(worldPath, 'level.dat');
  if (!fs.existsSync(levelDatPath)) return null;
  const nbt = require('prismarine-nbt');
  const raw = fs.readFileSync(levelDatPath);
  const { parsed } = await nbt.parse(raw); // auto-detects gzip vs raw
  const root = parsed.value.Data ? parsed.value.Data.value : parsed.value;
  const dv = root.DataVersion;
  return dv ? dv.value : null;
}

async function resolveMcVersion(worldAPath, forced) {
  if (forced) return forced;
  const dv = await readDataVersion(worldAPath);
  if (dv == null) {
    usageAndExit(
      `couldn't read a DataVersion from ${path.join(worldAPath, 'level.dat')} -- pass --mc-version explicitly`
    );
  }
  const mcData = require('minecraft-data');
  const versions = mcData.versionsByMinecraftVersion.pc;
  for (const v of Object.keys(versions)) {
    if (versions[v].dataVersion === dv) return v;
  }
  usageAndExit(`DataVersion ${dv} (from ${worldAPath}) has no known minecraft-data mapping -- pass --mc-version explicitly`);
}

function regionCoordsForChunk(cx, cz) {
  return { rx: cx >> 5, rz: cz >> 5 };
}

function regionFileExists(regionDir, cx, cz) {
  const { rx, rz } = regionCoordsForChunk(cx, cz);
  return fs.existsSync(path.join(regionDir, `r.${rx}.${rz}.mca`));
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args._.length < 2 || !args.bbox) usageAndExit();

  const [worldAArg, worldBArg] = args._;
  const worldA = path.resolve(worldAArg);
  const worldB = path.resolve(worldBArg);
  for (const w of [worldA, worldB]) {
    if (!fs.existsSync(w)) usageAndExit(`world path does not exist: ${w}`);
  }

  const bboxParts = args.bbox.split(',').map(Number);
  if (bboxParts.length !== 4 || bboxParts.some(Number.isNaN)) {
    usageAndExit('--bbox must be xMin,zMin,xMax,zMax (block coordinates)');
  }
  const [xMin, zMin, xMax, zMax] = bboxParts;
  const yMin = Number.isInteger(args.yMin) ? args.yMin : -64;
  const yMax = Number.isInteger(args.yMax) ? args.yMax : 320;
  const top = Number.isInteger(args.top) ? args.top : 20;

  const mcVersion = await resolveMcVersion(worldA, args.mcVersion);
  console.log(`anvil-diff: comparing as Minecraft ${mcVersion}`);
  console.log(`  A: ${worldA}`);
  console.log(`  B: ${worldB}`);
  console.log(`  bbox: x[${xMin},${xMax}] z[${zMin},${zMax}] y[${yMin},${yMax})`);

  const { Anvil } = require('prismarine-provider-anvil');
  const AnvilCtor = Anvil(mcVersion);
  const regionDirA = resolveRegionDir(worldA);
  const regionDirB = resolveRegionDir(worldB);
  const anvilA = new AnvilCtor(regionDirA);
  const anvilB = new AnvilCtor(regionDirB);

  const Block = require('prismarine-block')(mcVersion);

  const cxMin = xMin >> 4;
  const cxMax = xMax >> 4;
  const czMin = zMin >> 4;
  const czMax = zMax >> 4;

  let chunksCompared = 0;
  let chunksWithDiff = 0;
  let chunksSkippedBothAbsent = 0;
  let totalDiffBlocks = 0;
  const chunkReports = [];

  for (let cx = cxMin; cx <= cxMax; cx++) {
    for (let cz = czMin; cz <= czMax; cz++) {
      const hasA = regionFileExists(regionDirA, cx, cz);
      const hasB = regionFileExists(regionDirB, cx, cz);
      if (!hasA && !hasB) {
        chunksSkippedBothAbsent++;
        continue;
      }

      const chunkA = hasA ? await anvilA.load(cx, cz) : null;
      const chunkB = hasB ? await anvilB.load(cx, cz) : null;
      chunksCompared++;

      if (!chunkA && !chunkB) continue; // regions exist but chunk itself never generated on either side

      const localXMin = Math.max(0, xMin - cx * 16);
      const localXMax = Math.min(15, xMax - cx * 16);
      const localZMin = Math.max(0, zMin - cz * 16);
      const localZMax = Math.min(15, zMax - cz * 16);

      let diffCount = 0;
      let firstDiff = null;
      for (let y = yMin; y < yMax; y++) {
        for (let lx = localXMin; lx <= localXMax; lx++) {
          for (let lz = localZMin; lz <= localZMax; lz++) {
            const pos = { x: lx, y, z: lz };
            const idA = chunkA ? safeGetStateId(chunkA, pos) : 0; // 0 == air
            const idB = chunkB ? safeGetStateId(chunkB, pos) : 0;
            if (idA !== idB) {
              diffCount++;
              if (!firstDiff) {
                firstDiff = {
                  x: cx * 16 + lx,
                  y,
                  z: cz * 16 + lz,
                  a: blockName(Block, idA),
                  b: blockName(Block, idB),
                };
              }
            }
          }
        }
      }

      if (diffCount > 0) {
        chunksWithDiff++;
        totalDiffBlocks += diffCount;
        chunkReports.push({ cx, cz, diffCount, example: firstDiff });
      }
    }
  }

  chunkReports.sort((a, b) => b.diffCount - a.diffCount);

  console.log('');
  console.log(`anvil-diff summary:`);
  console.log(`  chunks compared: ${chunksCompared}`);
  console.log(`  chunks with a difference: ${chunksWithDiff}`);
  console.log(`  chunks absent on both sides (skipped): ${chunksSkippedBothAbsent}`);
  console.log(`  total differing block positions: ${totalDiffBlocks}`);
  if (chunkReports.length > 0) {
    console.log('');
    console.log(`  top ${Math.min(top, chunkReports.length)} chunk(s) by diff count:`);
    for (const r of chunkReports.slice(0, top)) {
      console.log(
        `    chunk (${r.cx},${r.cz}): ${r.diffCount} block(s) differ -- e.g. (${r.example.x},${r.example.y},${r.example.z}) ${r.example.a} -> ${r.example.b}`
      );
    }
  }

  if (args.json) {
    const summary = {
      worldA,
      worldB,
      bbox: { xMin, zMin, xMax, zMax, yMin, yMax },
      mcVersion,
      chunksCompared,
      chunksWithDiff,
      chunksSkippedBothAbsent,
      totalDiffBlocks,
      chunks: chunkReports,
    };
    fs.writeFileSync(args.json, JSON.stringify(summary, null, 2));
    console.log('');
    console.log(`wrote ${args.json}`);
  }
}

function safeGetStateId(chunk, pos) {
  try {
    return chunk.getBlockStateId(pos);
  } catch {
    return 0; // section not present at this Y -- treat as air
  }
}

function blockName(Block, stateId) {
  try {
    return Block.fromStateId(stateId, 0).name;
  } catch {
    return `state#${stateId}`;
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
