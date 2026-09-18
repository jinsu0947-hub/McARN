//! Road-network shortest-path routing for bus route polylines
//! (SPEC_GenerationScope.md §1.3): "링크 ID로 폴리라인을 구성하는 것이
//! 정확하지만, 없으면 정류소 순서를 따라 도로망 최단경로를 탐색해도
//! 무방하다" -- this project's route-stop mapping carries no link ID (see
//! `kr_bus_routes`'s module doc), so this is that fallback. That same
//! section also lowers the bar: "노선 폴리라인은 띠 범위 산출용이므로
//! 대략적 정확도로 충분하다" -- exact lane-level correctness isn't the
//! goal, just a reasonable centerline to buffer for `kr_transit`'s chunk
//! scoping.
//!
//! Reuses this module's own `RawNode`/`RawLink`/`load_raw`/`clip` (the same
//! 표준노드링크 parse M1 uses) rather than a second reader -- one shapefile
//! format, one loader.

use super::{clip, load_raw, RawLink, RawNode};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::Path;

fn polyline_length(points: &[(f64, f64)]) -> f64 {
    points.windows(2).map(|w| dist(w[0], w[1])).sum()
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// A clipped 표준노드링크 subgraph, ready for nearest-node lookup and
/// shortest-path search. Edges are undirected -- this module doc's own "no
/// oneway signal" gap (see `kr_roads`'s module doc on `oneway`) means a
/// directed graph could not be built correctly anyway, and an approximate
/// polyline has no need of one.
/// EN-metre width of a `nearest_node` grid cell. Not derived from the data --
/// just small enough that a typical query's expanding-ring search touches a
/// handful of cells (표준노드링크 nodes run a few tens of metres apart in
/// urban areas), large enough that the grid itself stays small in memory.
const GRID_CELL_SIZE_M: f64 = 250.0;

pub struct RoadGraph {
    /// Index -> node ID / position, built once so every `shortest_path` call
    /// reuses the same numbering instead of rebuilding it per query (this
    /// used to be per-call and dominated the runtime -- a single route's
    /// worth of hops rebuilding a ~10^4-10^5 node index each time).
    ids: Vec<String>,
    positions: Vec<(f64, f64)>,
    index_of: HashMap<String, usize>,
    /// node index -> [(neighbour index, edge length, link_id), ...]
    adjacency: Vec<Vec<(usize, f64, String)>>,
    /// `(floor(e/GRID_CELL_SIZE_M), floor(n/GRID_CELL_SIZE_M))` -> node
    /// indices in that cell -- `nearest_node`'s spatial index. A plain
    /// linear scan over every clipped node (once per stop, per route) was
    /// the dominant cost of building a route polyline; this replaces it
    /// with an expanding-ring search that only visits nearby cells.
    grid: HashMap<(i32, i32), Vec<usize>>,
}

fn grid_cell(e: f64, n: f64) -> (i32, i32) {
    ((e / GRID_CELL_SIZE_M).floor() as i32, (n / GRID_CELL_SIZE_M).floor() as i32)
}

impl RoadGraph {
    /// Loads and clips 표준노드링크 to `en_bbox` (EPSG:5186 metres), then
    /// builds the adjacency list. `en_bbox` should cover every stop any
    /// route this graph serves might need to reach -- a node outside it is
    /// simply absent, so a path that would have to leave the box comes back
    /// `None` rather than a route that silently jumps outside the clip.
    pub fn load(dir: &Path, en_bbox: (f64, f64, f64, f64)) -> Result<Self, String> {
        let (raw_nodes, raw_links) = load_raw(dir)?;
        // No scope filter here: routing between two stops legitimately needs
        // roads outside scope (SPEC_Scope §2 L1) to connect them -- only the
        // bbox clip (this graph's whole reason for a padded `en_bbox`, see
        // this fn's own doc) applies.
        let (nodes, links) = clip(raw_nodes, raw_links, en_bbox, None);

        let mut ids = Vec::with_capacity(nodes.len());
        let mut positions = Vec::with_capacity(nodes.len());
        let mut index_of = HashMap::with_capacity(nodes.len());
        for n in nodes {
            index_of.insert(n.id.clone(), ids.len());
            ids.push(n.id);
            positions.push((n.e, n.n));
        }

        let mut adjacency: Vec<Vec<(usize, f64, String)>> = vec![Vec::new(); ids.len()];
        for l in &links {
            let (Some(&fi), Some(&ti)) = (index_of.get(&l.f_node), index_of.get(&l.t_node)) else {
                continue;
            };
            let len = polyline_length(&l.points_en).max(1e-3);
            adjacency[fi].push((ti, len, l.link_id.clone()));
            adjacency[ti].push((fi, len, l.link_id.clone()));
        }

        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, &(e, n)) in positions.iter().enumerate() {
            grid.entry(grid_cell(e, n)).or_default().push(i);
        }

        Ok(Self { ids, positions, index_of, adjacency, grid })
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Nearest graph node to an arbitrary EN point, via [`Self::grid`]: scan
    /// cells in an expanding ring around the query point's own cell, stop
    /// once the best candidate found so far is provably closer than
    /// anything a wider ring could contain (`ring_radius * cell_size` is a
    /// lower bound on any not-yet-visited cell's distance to the query
    /// point).
    pub fn nearest_node(&self, e: f64, n: f64) -> Option<&str> {
        if self.positions.is_empty() {
            return None;
        }
        let (cx, cz) = grid_cell(e, n);
        let mut best: Option<(f64, usize)> = None;
        let mut radius: i32 = 0;
        loop {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    // Only the new ring's border -- interior cells were
                    // already scanned at smaller radii.
                    if radius > 0 && dx.abs() != radius && dz.abs() != radius {
                        continue;
                    }
                    if let Some(cell_nodes) = self.grid.get(&(cx + dx, cz + dz)) {
                        for &i in cell_nodes {
                            let d = dist(self.positions[i], (e, n));
                            if best.is_none_or(|(bd, _)| d < bd) {
                                best = Some((d, i));
                            }
                        }
                    }
                }
            }
            if let Some((bd, _)) = best {
                if (radius as f64) * GRID_CELL_SIZE_M >= bd {
                    break;
                }
            }
            radius += 1;
            // Safety bound: a clipped bbox a few thousand km across would
            // need on the order of 10^4 rings at this cell size -- far
            // beyond any real 표준노드링크 clip, so hitting this means the
            // grid is empty near the query point, not a slow convergence.
            if radius > 20_000 {
                break;
            }
        }
        best.map(|(_, i)| self.ids[i].as_str())
    }

    /// Dijkstra shortest path from `from` to `to` (node IDs). Returns the
    /// path's EN points (node positions, in order, both endpoints included)
    /// and the link IDs traversed (may repeat a link_id if it's genuinely
    /// crossed more than once, deduplication is the caller's call). `None`
    /// if either node is unknown or no path exists within the clipped graph.
    pub fn shortest_path(&self, from: &str, to: &str) -> Option<(Vec<(f64, f64)>, Vec<String>)> {
        if from == to {
            return self.index_of.get(from).map(|&i| (vec![self.positions[i]], Vec::new()));
        }
        let (&start, &goal) = (self.index_of.get(from)?, self.index_of.get(to)?);

        #[derive(PartialEq)]
        struct HeapEntry {
            cost: f64,
            node: usize,
        }
        impl Eq for HeapEntry {}
        impl Ord for HeapEntry {
            fn cmp(&self, other: &Self) -> Ordering {
                // Min-heap via reversed float compare (NaN can't occur --
                // edge lengths are always finite, non-negative).
                other.cost.total_cmp(&self.cost)
            }
        }
        impl PartialOrd for HeapEntry {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut dist_to = vec![f64::INFINITY; self.ids.len()];
        let mut prev: Vec<Option<(usize, String)>> = vec![None; self.ids.len()];
        let mut heap = BinaryHeap::new();
        dist_to[start] = 0.0;
        heap.push(HeapEntry { cost: 0.0, node: start });

        while let Some(HeapEntry { cost, node }) = heap.pop() {
            if node == goal {
                break;
            }
            if cost > dist_to[node] {
                continue;
            }
            for &(neighbour, weight, ref link_id) in &self.adjacency[node] {
                let next_cost = cost + weight;
                if next_cost < dist_to[neighbour] {
                    dist_to[neighbour] = next_cost;
                    prev[neighbour] = Some((node, link_id.clone()));
                    heap.push(HeapEntry { cost: next_cost, node: neighbour });
                }
            }
        }

        if dist_to[goal].is_infinite() {
            return None;
        }

        let mut path_nodes = vec![goal];
        let mut link_ids = Vec::new();
        let mut cur = goal;
        while cur != start {
            let (p, link_id) = prev[cur].clone().expect("reachable node must have a predecessor");
            link_ids.push(link_id);
            path_nodes.push(p);
            cur = p;
        }
        path_nodes.reverse();
        link_ids.reverse();

        let points = path_nodes.into_iter().map(|i| self.positions[i]).collect();
        Some((points, link_ids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_from_edges(edges: &[(&str, &str, f64, &str)]) -> RoadGraph {
        let mut ids: Vec<String> = Vec::new();
        let mut positions: Vec<(f64, f64)> = Vec::new();
        let mut index_of: HashMap<String, usize> = HashMap::new();
        let mut next_x = 0.0;
        let mut index_for = |id: &str, ids: &mut Vec<String>, positions: &mut Vec<(f64, f64)>, index_of: &mut HashMap<String, usize>, next_x: &mut f64| -> usize {
            *index_of.entry(id.to_string()).or_insert_with(|| {
                ids.push(id.to_string());
                positions.push((*next_x, 0.0));
                *next_x += 1.0;
                ids.len() - 1
            })
        };
        let mut adjacency: Vec<Vec<(usize, f64, String)>> = Vec::new();
        for &(a, b, len, link) in edges {
            let ai = index_for(a, &mut ids, &mut positions, &mut index_of, &mut next_x);
            let bi = index_for(b, &mut ids, &mut positions, &mut index_of, &mut next_x);
            while adjacency.len() < ids.len() {
                adjacency.push(Vec::new());
            }
            adjacency[ai].push((bi, len, link.to_string()));
            adjacency[bi].push((ai, len, link.to_string()));
        }
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, &(e, n)) in positions.iter().enumerate() {
            grid.entry(grid_cell(e, n)).or_default().push(i);
        }
        RoadGraph { ids, positions, index_of, adjacency, grid }
    }

    #[test]
    fn finds_the_direct_edge_when_it_is_shortest() {
        let g = graph_from_edges(&[("a", "b", 10.0, "L1"), ("a", "c", 3.0, "L2"), ("c", "b", 3.0, "L3")]);
        let (points, links) = g.shortest_path("a", "b").unwrap();
        assert_eq!(links, vec!["L2".to_string(), "L3".to_string()], "should prefer the 6-length detour over the 10-length direct edge");
        assert_eq!(points.len(), 3);
    }

    #[test]
    fn returns_none_for_disconnected_nodes() {
        let g = graph_from_edges(&[("a", "b", 1.0, "L1"), ("c", "d", 1.0, "L2")]);
        assert!(g.shortest_path("a", "d").is_none());
    }

    #[test]
    fn same_start_and_end_is_a_trivial_path() {
        let g = graph_from_edges(&[("a", "b", 1.0, "L1")]);
        let (points, links) = g.shortest_path("a", "a").unwrap();
        assert_eq!(points.len(), 1);
        assert!(links.is_empty());
    }

    #[test]
    fn nearest_node_finds_the_closest_point_across_a_grid_cell_boundary() {
        // graph_from_edges spaces nodes 1.0m apart on the x-axis, far inside
        // a single GRID_CELL_SIZE_M cell -- place the query point just past
        // node "c" (index 2, at x=2.0) so the true nearest node ("c" or "d")
        // and the ring search have to agree despite "b" (x=1.0) being a
        // plausible-looking first guess.
        let g = graph_from_edges(&[("a", "b", 1.0, "L1"), ("b", "c", 1.0, "L2"), ("c", "d", 1.0, "L3")]);
        assert_eq!(g.nearest_node(2.1, 0.0), Some("c"));
        assert_eq!(g.nearest_node(-0.4, 0.0), Some("a"));
    }

    #[test]
    fn nearest_node_searches_outward_across_cell_boundaries() {
        // Two nodes several grid cells apart -- the query point sits in an
        // empty cell between them, closer to the second. The ring search
        // must expand past its own starting (empty) cell to find either.
        let mut ids = vec!["near".to_string(), "far".to_string()];
        let mut positions = vec![(GRID_CELL_SIZE_M * 3.0, 0.0), (-GRID_CELL_SIZE_M * 5.0, 0.0)];
        let mut index_of = HashMap::new();
        index_of.insert("near".to_string(), 0);
        index_of.insert("far".to_string(), 1);
        let adjacency = vec![Vec::new(), Vec::new()];
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, &(e, n)) in positions.iter().enumerate() {
            grid.entry(grid_cell(e, n)).or_default().push(i);
        }
        let g = RoadGraph { ids: std::mem::take(&mut ids), positions: std::mem::take(&mut positions), index_of, adjacency, grid };
        // Query from the origin (an empty cell) -- "near" (3 cells away) must win over "far" (5 cells away).
        assert_eq!(g.nearest_node(0.0, 0.0), Some("near"));
    }
}
