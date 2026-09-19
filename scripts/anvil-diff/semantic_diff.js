const fs = require('fs');

function loadJSON(p) { return JSON.parse(fs.readFileSync(p, 'utf8')); }

function diffRoadgraph(aPath, bPath) {
  const a = loadJSON(aPath), b = loadJSON(bPath);
  console.log('roadgraph: bbox a=%s b=%s', JSON.stringify(a.bbox), JSON.stringify(b.bbox));
  console.log('roadgraph: nodes a=%d b=%d, segments a=%d b=%d', a.nodes.length, b.nodes.length, a.segments.length, b.segments.length);

  const sortedA_nodes = [...a.nodes].sort((x, y) => x.id.localeCompare(y.id));
  const sortedB_nodes = [...b.nodes].sort((x, y) => x.id.localeCompare(y.id));
  let nodeDiffs = 0;
  for (let i = 0; i < sortedA_nodes.length; i++) {
    if (JSON.stringify(sortedA_nodes[i]) !== JSON.stringify(sortedB_nodes[i])) {
      nodeDiffs++;
      if (nodeDiffs <= 5) console.log('  node diff:', JSON.stringify(sortedA_nodes[i]), 'vs', JSON.stringify(sortedB_nodes[i]));
    }
  }
  console.log('roadgraph: node diffs (sorted by id) =', nodeDiffs);

  const sortedA_segs = [...a.segments].sort((x, y) => x.id.localeCompare(y.id));
  const sortedB_segs = [...b.segments].sort((x, y) => x.id.localeCompare(y.id));
  let segDiffs = 0;
  for (let i = 0; i < sortedA_segs.length; i++) {
    const sa = JSON.stringify(sortedA_segs[i]);
    const sb = JSON.stringify(sortedB_segs[i]);
    if (sa !== sb) {
      segDiffs++;
      if (segDiffs <= 5) {
        // find first differing point for a compact report
        console.log('  segment diff at id', sortedA_segs[i].id, 'len a/b', sa.length, sb.length);
      }
    }
  }
  console.log('roadgraph: segment diffs (sorted by id) =', segDiffs, 'of', sortedA_segs.length);
}

function diffStops(aPath, bPath) {
  const a = loadJSON(aPath), b = loadJSON(bPath);
  const ak = Object.keys(a), bk = Object.keys(b);
  console.log('stops.json top-level keys:', ak, bk);
  for (const k of ak) {
    const av = a[k], bv = b[k];
    if (!Array.isArray(av)) {
      if (JSON.stringify(av) !== JSON.stringify(bv)) console.log(`  key ${k} differs (non-array):`, av, bv);
      continue;
    }
    if (av.length !== bv.length) {
      console.log(`  key ${k}: length differs a=${av.length} b=${bv.length}`);
      continue;
    }
    const keyFn = (o) => JSON.stringify(Object.keys(o).sort().map(kk => [kk, o[kk]]));
    const sa = [...av].sort((x, y) => keyFn(x).localeCompare(keyFn(y)));
    const sb = [...bv].sort((x, y) => keyFn(x).localeCompare(keyFn(y)));
    let diffs = 0;
    for (let i = 0; i < sa.length; i++) {
      if (JSON.stringify(sa[i]) !== JSON.stringify(sb[i])) diffs++;
    }
    console.log(`  key ${k}: ${av.length} entries, diffs after sort = ${diffs}`);
  }
}

const [cmd, aPath, bPath] = process.argv.slice(2);
if (cmd === 'roadgraph') diffRoadgraph(aPath, bPath);
else if (cmd === 'stops') diffStops(aPath, bPath);
else console.error('usage: node semantic_diff.js roadgraph|stops <a> <b>');
