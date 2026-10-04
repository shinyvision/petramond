use super::game::ServerGame;
use crate::{
    events::{tick::TickEvents, CellsEditPre, Outcome, PostEvent},
    world::cells::{CellEdit, CellHooks, CellPolicy, Cells},
};

pub(super) const CELLS_PER_TICK: usize = 8192;

impl ServerGame {
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
            mods,
            ..
        } = self;
        let actor = Some(sessions[s].id);
        let bus = mods.bus_mut();
        let refused =
            bus.cells_edit_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel;
        if refused {
            return Err((edit.finish().target, "This area cannot be edited".into()));
        }
        Ok(edit)
    }

    pub(super) fn step_cell_edit(
        &mut self,
        s: usize,
        edit: &mut CellEdit,
        events: &mut TickEvents,
    ) -> Result<bool, String> {
        let player = Some(self.sessions[s].id);
        let Self {
            world,
            sessions,
            mods,
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
                    player,
                });
            let placed = hooks
                .placed
                .iter()
                .map(|&(pos, block)| PostEvent::BlockPlaced { pos, block, player });
            for event in removed.chain(placed) {
                mods.emit(event);
            }
            mods.drain_posts(world, sessions, events);
        })
    }
}
