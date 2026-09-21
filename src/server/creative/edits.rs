//! Edits through the cell applicator: schematic placement, history replay,
//! and the per-tick record of single-block operator edits.

use super::history::{EditRecord, Replay};
use crate::{
    events::tick::TickEvents,
    schematic::Schematic,
    server::game::ServerGame,
    world::cells::{CellEdit, CellPolicy},
};
use petramond_math::math::IVec3;

/// An edit being written over several ticks, and what its receipt becomes.
pub(super) struct EditJob {
    edit: CellEdit,
    kind: JobKind,
}

enum JobKind {
    Placement { update_bounds: [IVec3; 2] },
    Replay { record: EditRecord, replay: Replay },
}

impl ServerGame {
    pub(super) fn place_schematic(
        &mut self,
        s: usize,
        schematic: &Schematic,
        origin: [i32; 3],
        turns: u8,
        events: &mut TickEvents,
    ) -> Result<(), String> {
        let origin = IVec3::from_array(origin);
        if (petramond_math::world_pos::WorldPos::block_center(origin)
            + schematic.placement_pivot(turns).as_vec3()
            - crate::server::movement::reach_eye(&self.sessions[s]))
        .length()
            > crate::schematic::PLACEMENT_REACH + 2.0
        {
            return Err("Place the schematic within reach".into());
        }
        let cells = schematic.placed_cells(origin, turns)?;
        let update_bounds = [
            origin - IVec3::ONE,
            origin + IVec3::from_array(schematic.rotated_size(turns)),
        ];
        self.close_open_edit(s, events);
        let policy = CellPolicy {
            record: true,
            update_bounds: Some(update_bounds),
        };
        let edit = self
            .begin_cell_edit(s, cells, policy, events)
            .map_err(|(_, message)| message)?;
        self.sessions[s].creative.job = Some(EditJob {
            edit,
            kind: JobKind::Placement { update_bounds },
        });
        self.step_edit_job(s, events)
    }

    pub(super) fn replay_edit(
        &mut self,
        s: usize,
        replay: Replay,
        events: &mut TickEvents,
    ) -> Result<(), String> {
        self.close_open_edit(s, events);
        let mut record = self.sessions[s]
            .edits
            .take(replay)
            .ok_or_else(|| format!("Nothing to {}", replay.verb()))?;
        let policy = CellPolicy {
            record: false,
            update_bounds: record.update_bounds.filter(|_| replay == Replay::Redo),
        };
        match self.begin_cell_edit(s, record.take_side(replay), policy, events) {
            Ok(edit) => {
                self.sessions[s].creative.job = Some(EditJob {
                    edit,
                    kind: JobKind::Replay { record, replay },
                });
                self.step_edit_job(s, events)
            }
            Err((cells, message)) => {
                record.put_side(replay, cells);
                self.sessions[s].edits.settle(record, replay, false);
                Err(message)
            }
        }
    }

    /// Write this tick's share of the session's edit; a finished or
    /// interrupted edit is filed in its history.
    pub(super) fn step_edit_job(
        &mut self,
        s: usize,
        events: &mut TickEvents,
    ) -> Result<(), String> {
        let Some(mut job) = self.sessions[s].creative.job.take() else {
            return Ok(());
        };
        let outcome = self.step_cell_edit(&mut job.edit, events);
        if outcome == Ok(false) {
            self.sessions[s].creative.job = Some(job);
            return Ok(());
        }
        let mut receipt = job.edit.finish();
        let history = &mut self.sessions[s].edits;
        match job.kind {
            // An interrupted placement records what it did write, so undo
            // still takes back exactly that.
            JobKind::Placement { update_bounds } => {
                receipt.target.truncate(receipt.written);
                receipt.target.append(&mut receipt.cleared);
                if !receipt.target.is_empty() {
                    history.record(receipt.before, receipt.target, Some(update_bounds));
                }
            }
            // A replay is unconditional, so an interrupted one is simply
            // retried from its original stack.
            JobKind::Replay { mut record, replay } => {
                record.put_side(replay, receipt.target);
                history.settle(record, replay, outcome.is_ok());
            }
        }
        outcome.map(|_| ())
    }

    /// Note cells session `s` is about to change outside the applicator, so
    /// single-block edits share the history bulk edits use.
    pub(in crate::server) fn touch_edit_cells(
        &mut self,
        s: usize,
        cells: impl IntoIterator<Item = IVec3>,
    ) {
        if !self.sessions[s].player.abilities().edits_cells {
            return;
        }
        let Self {
            world, sessions, ..
        } = self;
        for pos in cells {
            sessions[s]
                .edits
                .touch(pos, || world.snapshot_cell(pos).ok());
        }
    }

    /// File the cells touched since the last close as one edit, once the
    /// block hooks they queued have settled.
    pub(in crate::server) fn close_open_edit(&mut self, s: usize, events: &mut TickEvents) {
        if !self.sessions[s].edits.has_open() {
            return;
        }
        self.drain_post_events(events);
        let Self {
            world, sessions, ..
        } = self;
        sessions[s].edits.close(|pos| world.snapshot_cell(pos).ok());
    }
}
