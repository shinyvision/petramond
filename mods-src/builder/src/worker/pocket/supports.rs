use crate::fx::{HashMap, HashSet};
use crate::geometry::offset;
use crate::worker::tuning::reach::MAX_HEIGHT;

use super::graph::{Cell, Graph};

pub(super) fn columns(
    graph: &Graph,
    raw: &[Cell],
    governed: &HashSet<[i32; 3]>,
) -> HashMap<usize, usize> {
    let mut columns: HashMap<usize, usize> = HashMap::default();
    for &base in &graph.ground {
        let foot = graph.bounds.cell(base);
        if governed.contains(&foot) {
            continue;
        }
        for height in 1..=MAX_HEIGHT {
            let cell = offset(foot, [0, height, 0]);
            let Some(node) = graph.bounds.index(cell) else {
                break;
            };
            let below = graph.bounds.index(offset(cell, [0, -1, 0])).unwrap();
            if !graph.cells[node].open
                || !raw[below].open
                || governed.contains(&cell)
                || governed.contains(&offset(cell, [0, 1, 0]))
            {
                break;
            }
            columns
                .entry(node)
                .and_modify(|b| *b = (*b).max(base))
                .or_insert(base);
        }
    }
    columns
}

#[cfg(test)]
mod tests;
