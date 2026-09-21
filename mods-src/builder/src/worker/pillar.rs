//! Reaching high work: the golem jumps and builds a scaffold block into the
//! cell it just left, repeatedly, until it stands high enough, works from
//! the top, then digs the pillar out from under itself on the way down.
//!
//! A pillar never stands in a cell the design governs, so the scaffolding
//! can neither block a unit nor be mistaken for one.

mod climb;
mod find;
mod onward;

pub use climb::{climb, descend, recover};
pub use find::find;
pub(super) use find::lays;
pub use onward::find_onward;

use mod_sdk::*;

use super::tuning::reach::MAX_HEIGHT;
use super::{open_block, Body, Ctx};
use crate::fx::HashSet;
use crate::geometry::reaches;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pillar {
    pub column: [i32; 2],
    /// Feet level at the bottom, where the first scaffold goes.
    pub base: i32,
    /// Feet level at the top.
    pub top: i32,
    /// A foothold beside the base: where the golem is once it is down.
    pub exit: [i32; 3],
    /// Standing room the golem walks on to from the top (along a roof course)
    /// to work what no perch sees.
    pub onward: Option<[i32; 3]>,
}

impl Pillar {
    /// Where the golem stands at the top, and at the foot before the climb.
    pub fn top_cell(&self) -> [i32; 3] {
        [self.column[0], self.top, self.column[1]]
    }

    pub fn foot_cell(&self) -> [i32; 3] {
        [self.column[0], self.base, self.column[1]]
    }

    /// Whether `cell` lies in the pillar's column, at any height.
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
