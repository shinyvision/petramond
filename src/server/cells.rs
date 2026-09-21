//! The server's bulk cell-edit funnel: announce the edit, then write it in
//! budgeted slices with the block hooks bracketed around each one.

use super::game::ServerGame;
use crate::{
    events::{tick::TickEvents, CellsEditPre, Outcome, PostEvent},
    world::cells::{CellEdit, CellHooks, CellPolicy, Cells},
};

/// Cells one edit writes per tick.
pub(super) const CELLS_PER_TICK: usize = 8192;

/// Post events emitted between drains, well under the bus's per-drain bound
/// so a slice of any size never has its hooks dropped.
const HOOKS_PER_DRAIN: usize = 1024;

impl ServerGame {
    /// Validate and announce an edit by session `s`. A refusal hands the
    /// cells back untouched.
    pub(super) fn begin_cell_edit(
        &mut self,
        s: usize,
        target: Cells,
        policy: CellPolicy,
        events: &mut TickEvents,
    ) -> Result<CellEdit, (Cells, String)> {
        let edit = self.world.begin_cells(target, policy)?;
        let [min, max] = edit.bounds();
        let mut pre = CellsEditPre {
            min,
            max,
            cells: edit.len(),
            actor: crate::mob::EntityRef::Player(self.sessions[s].id),
        };
        let Self {
            world,
            sessions,
            bus,
            ..
        } = self;
        let refused = Self::with_sessions_view(sessions, s, |sess| {
            bus.cells_edit_pre(
                world,
                &mut sess.player,
                &mut sess.gui_state,
                events,
                &mut pre,
            ) == Outcome::Cancel
        });
        if refused {
            return Err((edit.finish().target, "This area cannot be edited".into()));
        }
        Ok(edit)
    }

    /// Write this tick's share of `edit`. Returns whether it is complete.
    pub(super) fn step_cell_edit(
        &mut self,
        edit: &mut CellEdit,
        events: &mut TickEvents,
    ) -> Result<bool, String> {
        let Self {
            world,
            sessions,
            bus,
            ..
        } = self;
        world.step_cells(edit, CELLS_PER_TICK, &mut |world, hooks: CellHooks<'_>| {
            let removed = hooks
                .removed
                .iter()
                .map(|&(pos, block)| PostEvent::BlockBroken {
                    pos,
                    block,
                    harvested: false,
                    natural: false,
                });
            let placed = hooks
                .placed
                .iter()
                .map(|&(pos, block)| PostEvent::BlockPlaced { pos, block });
            let mut queued = 0;
            for event in removed.chain(placed) {
                bus.emit(event);
                queued += 1;
                if queued % HOOKS_PER_DRAIN == 0 {
                    Self::with_sessions_view(sessions, 0, |host| {
                        bus.drain_post(world, &mut host.player, &mut host.gui_state, events)
                    });
                }
            }
            Self::with_sessions_view(sessions, 0, |host| {
                bus.drain_post(world, &mut host.player, &mut host.gui_state, events)
            });
        })
    }
}
