use crate::fx::HashSet;
use crate::worker::tuning::reach::MAX_HEIGHT;

use super::graph::{Cell, Graph};
use super::grid::Bounds;

const NONE: u32 = u32::MAX;

/// For every cell a scaffold column could hold up, the reachable dry foothold it rises from.
#[derive(Default)]
pub(super) struct Supports(Vec<u32>);

impl Supports {
    pub fn get(&self, cell: usize) -> Option<usize> {
        self.0
            .get(cell)
            .copied()
            .filter(|base| *base != NONE)
            .map(|base| base as usize)
    }

    pub fn contains(&self, cell: usize) -> bool {
        self.get(cell).is_some()
    }
}

/// The cells of `bounds` the design governs.
pub(super) fn governed_cells(bounds: Bounds, governed: &HashSet<[i32; 3]>) -> Vec<bool> {
    let mut shut = vec![false; bounds.len()];
    for cell in governed {
        if let Some(i) = bounds.index(*cell) {
            shut[i] = true;
        }
    }
    shut
}

/// `shut` marks the cells the design governs, as `governed_cells` gives them.
pub(super) fn columns(graph: &Graph, raw: &[Cell], shut: &[bool]) -> Supports {
    let len = graph.cells.len();
    let layer = graph.bounds.size[0] * graph.bounds.size[2];
    let mut columns = vec![NONE; len];
    for base in (0..len).filter(|i| graph.ground[*i]) {
        if shut[base] {
            continue;
        }
        // Cells are laid out in layers, so the cell above is one layer on.
        let mut node = base;
        for _ in 1..=MAX_HEIGHT {
            node += layer;
            if node >= len {
                break;
            }
            let above_shut = node + layer < len && shut[node + layer];
            if !graph.cells[node].open || !raw[node - layer].open || shut[node] || above_shut {
                break;
            }
            let held = &mut columns[node];
            if *held == NONE || base as u32 > *held {
                *held = base as u32;
            }
        }
    }
    Supports(columns)
}

#[cfg(test)]
mod tests;
