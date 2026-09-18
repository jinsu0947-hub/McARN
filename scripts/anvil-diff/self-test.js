// Builds two tiny synthetic Anvil worlds with a known, hand-placed
// difference, runs anvil-diff.js against them as a child process, and
// asserts the printed summary matches what was actually written -- so this
// tool's own correctness doesn't rest on "it didn't crash" alone.
//
// Run: node self-test.js

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const zlib = require('zlib');
const { execFileSync } = require('child_process');
const nbt = require('prismarine-nbt');

const MC_VERSION = '1.21.1';
const DATA_VERSION = 3955; // matches src/world_editor/java.rs's DATA_VERSION

function writeLevelDat(worldDir) {
  const tag = nbt.comp({
    Data: nbt.comp({
      DataVersion: nbt.int(DATA_VERSION),
      LevelName: nbt.string('anvil-diff-self-test'),
    }),
  });
  const buf = nbt.writeUncompressed(tag, 'big');
  fs.writeFileSync(path.join(worldDir, 'level.dat'), zlib.gzipSync(buf));
}

function buildWorld(worldDir, { stonePos, extraChunk }) {
  fs.mkdirSync(path.join(worldDir, 'region'), { recursive: true });
  writeLevelDat(worldDir);

  const { Anvil } = require('prismarine-provider-anvil');
  const AnvilCtor = Anvil(MC_VERSION);
  const PChunk = require('prismarine-chunk')(MC_VERSION);

  return (async () => {
    const anvil = new AnvilCtor(path.join(worldDir, 'region'));

    const chunk00 = new PChunk();
    chunk00.initialize(() => null); // air everywhere
    // 1 = stone's state id in every modern version (id 0 reserved for air).
    chunk00.setBlockStateId(stonePos, 1);
    await anvil.save(0, 0, chunk00);

    if (extraChunk) {
      const chunk10 = new PChunk();
      chunk10.initialize(() => null);
      chunk10.setBlockStateId({ x: 0, y: 0, z: 0 }, 1);
      await anvil.save(1, 0, chunk10);
    }

    await anvil.close();
  })();
}

async function main() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'anvil-diff-self-test-'));
  const worldA = path.join(tmp, 'worldA');
  const worldB = path.join(tmp, 'worldB');
  fs.mkdirSync(worldA);
  fs.mkdirSync(worldB);

  // A: stone at (5,0,5) in chunk (0,0), plus a second chunk (1,0) B lacks
  // entirely (region-absent-on-one-side path).
  // B: stone at (6,0,5) instead -- one block moved, a clean known diff.
  await buildWorld(worldA, { stonePos: { x: 5, y: 0, z: 5 }, extraChunk: true });
  await buildWorld(worldB, { stonePos: { x: 6, y: 0, z: 5 }, extraChunk: false });

  const jsonOut = path.join(tmp, 'summary.json');
  const output = execFileSync(
    process.execPath,
    [
      path.join(__dirname, 'anvil-diff.js'),
      worldA,
      worldB,
      '--bbox',
      '0,0,31,15', // chunks (0,0) and (1,0)
      '--y-min',
      '0',
      '--y-max',
      '1',
      '--json',
      jsonOut,
    ],
    { encoding: 'utf8' }
  );
  console.log(output);

  const summary = JSON.parse(fs.readFileSync(jsonOut, 'utf8'));

  const checks = [
    ['chunksCompared === 2', summary.chunksCompared === 2],
    ['chunksWithDiff === 2', summary.chunksWithDiff === 2], // (0,0) has the moved stone; (1,0) is stone-vs-air
    ['chunk (0,0) has exactly 2 differing positions (old spot air-vs-stone, new spot stone-vs-air)',
      summary.chunks.find((c) => c.cx === 0 && c.cz === 0)?.diffCount === 2],
    ['chunk (1,0) has exactly 1 differing position (A has stone, B has nothing)',
      summary.chunks.find((c) => c.cx === 1 && c.cz === 0)?.diffCount === 1],
    ['totalDiffBlocks === 3', summary.totalDiffBlocks === 3],
  ];

  let failed = false;
  for (const [desc, ok] of checks) {
    console.log(`${ok ? 'PASS' : 'FAIL'}: ${desc}`);
    if (!ok) failed = true;
  }

  fs.rmSync(tmp, { recursive: true, force: true });

  if (failed) {
    console.error('\nself-test FAILED');
    process.exit(1);
  }
  console.log('\nself-test passed');
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
