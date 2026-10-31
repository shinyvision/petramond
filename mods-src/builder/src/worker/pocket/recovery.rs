use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::graph::Graph;
use crate::fx::{HashMap, HashSet};
use crate::geometry::offset;
use crate::worker::Task;

#[derive(Clone, Copy, Debug)]
pub(super) struct Gate {
    pub task: Task,
    pub cell: [i32; 3],
    pub serves: u32,
    pub cost: u32,
    pub exposed: bool,
}

pub(super) fn plan(graph: &Graph) -> Vec<Gate> {
    if graph.sealed.is_empty() || !graph.reached[graph.root] {
        return Vec::new();
    }
    // Collapse connected air into single vertices so empty space has no search cost.
    let mut vertices = graph.components.clone();
    let mut digs = vec![None; graph.component_count];
    for (i, cell) in graph.cells.iter().enumerate() {
        if !cell.open && (cell.dig.is_some() || cell.head.is_some()) {
            vertices[i] = digs.len();
            digs.push(Some((i, *cell)));
        }
    }
    let mut edges = vec![Vec::new(); digs.len()];
    for (i, &vertex) in vertices.iter().enumerate() {
        if vertex == usize::MAX {
            continue;
        }
        for next in graph.bounds.neighbours(i).into_iter().flatten() {
            let other = vertices[next];
            if other != usize::MAX && other != vertex {
                edges[vertex].push(other);
            }
        }
    }
    for edges in &mut edges {
        edges.sort_unstable();
        edges.dedup();
    }
    let root = vertices[graph.root];
    let mut distance = vec![u32::MAX; digs.len()];
    let mut parent = vec![usize::MAX; digs.len()];
    distance[root] = 0;
    let mut queue = BinaryHeap::from([Reverse((0u32, root))]);
    while let Some(Reverse((cost, vertex))) = queue.pop() {
        if cost != distance[vertex] {
            continue;
        }
        for &next in &edges[vertex] {
            let step = digs[next].map_or(0, |(_, cell)| {
                cell.dig
                    .map_or(0, |(_, c)| c)
                    .saturating_add(cell.head.map_or(0, |(_, c)| c))
            });
            let cost = cost.saturating_add(step);
            if cost < distance[next] {
                distance[next] = cost;
                parent[next] = vertex;
                queue.push(Reverse((cost, next)));
            }
        }
    }
    let mut gates: HashMap<Task, Gate> = HashMap::default();
    for &sealed in &graph.sealed {
        let work = &graph.work[sealed];
        let goal = work
            .contacts
            .iter()
            .map(|i| vertices[*i])
            .filter(|v| *v != usize::MAX && distance[*v] != u32::MAX)
            .min_by_key(|v| (distance[*v], *v));
        let Some(mut vertex) = goal else { continue };
        let mut counted = HashSet::default();
        while vertex != root {
            if let Some((index, cell)) = digs[vertex] {
                for (action, pos) in [
                    (cell.dig, graph.bounds.cell(index)),
                    (cell.head, offset(graph.bounds.cell(index), [0, 1, 0])),
                ] {
                    let Some((task, cost)) = action else { continue };
                    if counted.insert(task) {
                        let gate = gates.entry(task).or_insert_with(|| Gate {
                            task,
                            cell: pos,
                            serves: 0,
                            cost,
                            exposed: graph.bounds.index(pos).is_some_and(|i| {
                                graph
                                    .bounds
                                    .neighbours(i)
                                    .into_iter()
                                    .flatten()
                                    .any(|n| graph.reached[n])
                            }),
                        });
                        gate.serves += 1;
                    }
                }
            }
            vertex = parent[vertex];
        }
    }
    let mut gates: Vec<Gate> = gates.into_values().collect();
    rank(&mut gates);
    gates
}

pub(super) fn rank(gates: &mut [Gate]) {
    gates.sort_by(|a, b| {
        (u64::from(b.serves) * u64::from(a.cost))
            .cmp(&(u64::from(a.serves) * u64::from(b.cost)))
            .then(a.cell.cmp(&b.cell))
    });
}
