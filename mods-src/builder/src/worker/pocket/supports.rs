use crate::fx::{HashMap, HashSet};
use crate::geometry::offset;
use crate::worker::tuning::reach::MAX_HEIGHT;

use super::graph::{Cell, Graph};

pub(super) fn columns(
    graph: &Graph,
    raw: &[Cell],
    governed: &HashSet<[i32; 3]>,
) -> HashMap<usize, usize> {
    let mut shut = vec![false; graph.cells.len()];
    for cell in governed {
        if let Some(i) = graph.bounds.index(*cell) {
            shut[i] = true;
        }
    }
    let shut = |cell: [i32; 3]| graph.bounds.index(cell).is_some_and(|i| shut[i]);
    let mut columns: HashMap<usize, usize> = HashMap::default();
    for &base in &graph.ground {
        let foot = graph.bounds.cell(base);
        if shut(foot) {
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
                || shut(cell)
                || shut(offset(cell, [0, 1, 0]))
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
