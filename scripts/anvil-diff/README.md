# anvil-diff

Chunk-level block diff between two Java Anvil worlds. Replaces the
one-off scripts `PROGRESS.md` §5 used to say got "그때그때 짜서" for every
Anvil-level verification — this one is meant to be reused.

## Setup

```bash
cd scripts/anvil-diff
npm install
npm test   # self-test against two synthetic worlds it builds itself, no real world needed
```

## Usage

```bash
node anvil-diff.js <worldA> <worldB> --bbox xMin,zMin,xMax,zMax [options]
```

`<worldA>`/`<worldB>` are Java Anvil world folders (with `region/*.mca`, or
pointed straight at a region folder). The bbox is block coordinates in
world space, not chunk coordinates.

Options:

| Flag | Default | Meaning |
|---|---|---|
| `--y-min` | `-64` | Lowest Y compared |
| `--y-max` | `320` | Highest Y compared (exclusive) |
| `--mc-version` | read from worldA's `level.dat` `DataVersion` | Force a `minecraft-data` version string |
| `--top` | `20` | How many differing chunks to print in detail |
| `--json <path>` | — | Also write the full summary as JSON |

Output is a chunk-granularity summary — total chunks compared, how many
differ, total differing block positions, and for the top N differing
chunks one representative coordinate + the two block names there — not a
full per-block listing. If you need the exhaustive list, read the
`--json` output's `chunks[].example` structure and adapt.

**Read-only.** A region file that doesn't exist on one (or both) sides is
treated as "every chunk in it is absent" (air) without ever touching it —
`prismarine-provider-anvil`'s own `Anvil.getRegion` otherwise *creates* a
region file on read if one is missing, which would corrupt this into a
mutating tool. `anvil-diff.js` checks `fs.existsSync` on the region file
itself before going anywhere near the library for it.

## Typical use: comparing two generation runs

```bash
node anvil-diff.js \
  path/to/rect-only/world \
  path/to/rect-plus-508/world \
  --bbox <영도 사각형의 블록 좌표 xMin,zMin,xMax,zMax> \
  --json rect-scope-diff.json
```

For the scope-generation-range validation `PROGRESS.md` §6 describes
(사각형 구간은 baseline과 일치, 508 띠 구간은 추가 생성, 손실 0): run this
against the *same bbox as the rect-only run* first (expect
`totalDiffBlocks: 0`), then separately confirm the 508-corridor tiles
exist in the second world's `region/` folder at all (a bbox check, not
this tool's job) — this tool only tells you whether two worlds *agree*
inside a shared bbox, not whether one has generated extra area the other
never had.
