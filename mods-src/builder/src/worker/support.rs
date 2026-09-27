use crate::fx::HashSet;

use crate::host::prelude::*;

use super::tuning::reach::SUPPORT_DEPTH;
use super::Ctx;
use crate::design::Design;
use crate::geometry::offset;

pub enum Support {
    Needed([i32; 3]),
    Standing,
    Impossible,
}

pub fn below(ctx: &mut Ctx, design: &Design, pos: [i32; 3]) -> Support {
    let column: Vec<[i32; 3]> = (1..=SUPPORT_DEPTH)
        .map(|d| offset(pos, [0, -d, 0]))
        .collect();
    let blocks = get_blocks(column.clone());
    for (d, (cell, block)) in column.iter().zip(blocks).enumerate() {
        let Some(block) = block else {
            return Support::Impossible;
        };
        if !super::open_block(ctx, block) {
            return if d == 0 {
                Support::Standing
            } else {
                Support::Needed(column[d - 1])
            };
        }
        if design.governed.contains(cell) {
            return Support::Impossible;
        }
    }
    Support::Impossible
}

pub fn props_up(cell: [i32; 3], open: &HashSet<[i32; 3]>) -> bool {
    const SIDES_AND_UP: [[i32; 3]; 5] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1], [0, 1, 0]];
    (1..=SUPPORT_DEPTH).any(|d| open.contains(&offset(cell, [0, d, 0])))
        || SIDES_AND_UP
            .iter()
            .any(|f| open.contains(&offset(cell, *f)))
}
