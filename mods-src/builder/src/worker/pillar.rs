mod climb;
mod find;
mod onward;

pub use climb::{climb, descend, recover};
pub use find::find;
pub(super) use find::lays;
pub use onward::find_onward;

use crate::host::prelude::*;

use super::tuning::reach::MAX_HEIGHT;
use super::{open_block, Body, Ctx};
use crate::fx::HashSet;
use crate::geometry::reaches;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pillar {
    pub column: [i32; 2],
    pub base: i32,
    pub top: i32,
    pub exit: [i32; 3],
    pub onward: Option<[i32; 3]>,
}

impl Pillar {
    pub fn top_cell(&self) -> [i32; 3] {
        [self.column[0], self.top, self.column[1]]
    }

    pub fn foot_cell(&self) -> [i32; 3] {
        [self.column[0], self.base, self.column[1]]
    }

    pub fn on_column(&self, cell: [i32; 3]) -> bool {
        self.column == [cell[0], cell[2]]
    }

    pub fn holds(&self, cell: [i32; 3]) -> bool {
        self.on_column(cell) && (self.base..self.top).contains(&cell[1])
    }

    pub fn extends_to(
        &self,
        ctx: &mut Ctx,
        body: &Body,
        cells: &[[i32; 3]],
        governed: &HashSet<[i32; 3]>,
    ) -> bool {
        let head = [self.column[0], self.top + 2, self.column[1]];
        let raised = [body.pos[0], body.pos[1] + 1.0, body.pos[2]];
        self.top - self.base < MAX_HEIGHT
            && cells.iter().all(|c| c[1] > self.top + 1)
            && reaches(raised, cells)
            && !governed.contains(&head)
            && !cells.contains(&head)
            && get_block(head).is_some_and(|b| open_block(ctx, b))
    }
}
