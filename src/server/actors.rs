//! Actions a mob performs on the world for a mod, at their turn in the
//! tick: the queued break of a finished dig and a construction placement.
//! Each is re-proven against the world as it now stands through the same
//! checks the request passed, runs through the engine's own funnels (the
//! break funnel, `block_place_pre`, the placement commit), and reports its
//! outcome as `actor_acted`.

use mod_api::{ActionRefusal, ActorAction};
use petramond_math::math::IVec3;
use petramond_world::construction::Record;
use petramond_world::mining::BreakEvent;

use super::breaking::Breaker;
use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::events::{BlockPlacePre, Outcome, PostEvent};
use crate::mob::EntityRef;
use crate::world::actor::{DigTarget, PlaceCheck, Placement};

impl ServerGame {
    pub(super) fn apply_actor_break(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        target: DigTarget,
        tool_slot: Option<u32>,
        collect: bool,
        events: &mut TickEvents,
    ) {
        let refusal = self
            .actor_break(mob_id, pos, target, tool_slot, collect, events)
            .err();
        self.bus.emit(PostEvent::ActorActed {
            actor: EntityRef::Mob(mob_id),
            pos,
            action: ActorAction::Dig,
            refusal,
        });
    }

    fn actor_break(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        target: DigTarget,
        tool_slot: Option<u32>,
        collect: bool,
        events: &mut TickEvents,
    ) -> Result<(), ActionRefusal> {
        // Pre-event handlers dispatch as a session; with nobody connected
        // there is no world streamed in to act on either.
        if self.sessions.is_empty() {
            return Err(ActionRefusal::Unloaded);
        }
        let actor = self.world.actor(mob_id)?;
        let now = self.world.dig_check(&actor, pos, tool_slot)?;
        if now != target {
            return Err(ActionRefusal::Changed);
        }
        let tool = now.tool.and_then(|t| t.tool());
        let event = BreakEvent {
            pos,
            block: now.block,
            harvested: petramond_world::mining::harvests(now.block, tool),
        };
        let breaker = Breaker {
            acting: 0,
            actor: EntityRef::Mob(mob_id),
            yields: true,
            collector: collect.then_some(mob_id),
            normal: None,
        };
        if self.break_block(breaker, event, events) {
            Ok(())
        } else {
            Err(ActionRefusal::Vetoed)
        }
    }

    /// A mob's use of the block at `pos`, re-proven at its turn and reported
    /// like its digs and placements.
    pub(super) fn apply_actor_interact(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        events: &mut TickEvents,
    ) {
        let refusal = self.actor_use(mob_id, pos, events).err();
        self.bus.emit(PostEvent::ActorActed {
            actor: EntityRef::Mob(mob_id),
            pos,
            action: ActorAction::Use,
            refusal,
        });
    }

    /// What a body with no session can use: the block's own capability where
    /// it asks nothing of a player (a door swings; a screen needs someone to
    /// show it to).
    fn actor_use(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        events: &mut TickEvents,
    ) -> Result<(), ActionRefusal> {
        let actor = self.world.actor(mob_id)?;
        self.world.block_click(&actor, pos)?;
        let block = self
            .world
            .block_if_stream_final(pos.x, pos.y, pos.z)
            .ok_or(ActionRefusal::Unloaded)?;
        match block.interaction() {
            petramond_world::block::BlockInteraction::ToggleDoor => self
                .swing_door(pos, events)
                .map(drop)
                .ok_or(ActionRefusal::NothingToDo),
            _ => Err(ActionRefusal::NothingToDo),
        }
    }

    pub(super) fn apply_actor_place(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        record: Record,
        pay: bool,
        events: &mut TickEvents,
    ) {
        let refusal = self.actor_place(mob_id, pos, &record, pay, events).err();
        self.bus.emit(PostEvent::ActorActed {
            actor: EntityRef::Mob(mob_id),
            pos,
            action: ActorAction::Place,
            refusal,
        });
    }

    fn actor_place(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        record: &Record,
        pay: bool,
        events: &mut TickEvents,
    ) -> Result<(), ActionRefusal> {
        if self.sessions.is_empty() {
            return Err(ActionRefusal::Unloaded);
        }
        let ready = self.check_actor_place(mob_id, pos, record, pay)?;
        let block = ready
            .plan
            .writes
            .iter()
            .find(|w| w.cell == ready.plan.anchor)
            .map_or(record.block, |w| w.block);
        let mut pre = BlockPlacePre {
            pos: ready.plan.anchor,
            block,
            facing: ready.click.facing,
            actor: EntityRef::Mob(mob_id),
        };
        let cancelled = {
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            Self::with_sessions_view(sessions, 0, |host| {
                bus.block_place_pre(
                    world,
                    &mut host.player,
                    &mut host.gui_state,
                    events,
                    &mut pre,
                ) == Outcome::Cancel
            })
        };
        if cancelled {
            return Err(ActionRefusal::Vetoed);
        }
        // Handlers may have changed the world or the actor: the placement
        // commits only exactly as it was proven.
        if self.check_actor_place(mob_id, pos, record, pay)? != ready {
            return Err(ActionRefusal::Changed);
        }
        let Placement {
            plan: writes, paid, ..
        } = ready;
        if !self.world.commit_placement(&writes, true) {
            return Err(ActionRefusal::Unloaded);
        }
        let carried = paid.item.as_block().unwrap_or(block);
        self.world
            .carry_into_cell(&paid, carried, writes.anchor, writes.anchor_part());
        if pay {
            let index = self.world.actor(mob_id)?.index;
            let paid = self
                .world
                .mobs_mut()
                .container_mut(index)
                .is_some_and(|c| c.take_all(std::slice::from_ref(&paid)));
            debug_assert!(paid, "the carried cost was proven just before the commit");
        }
        events.world.block_placed.push((writes.anchor, block));
        self.bus.emit(PostEvent::BlockPlaced {
            pos: writes.anchor,
            block,
        });
        self.push_noise_from(
            EntityRef::Mob(mob_id),
            writes.anchor,
            crate::mob::NoiseKind::BlockPlaced,
        );
        Ok(())
    }

    /// The placement check at the drain: every connected body counts.
    fn check_actor_place(
        &self,
        mob_id: u64,
        pos: IVec3,
        record: &Record,
        pay: bool,
    ) -> Result<Placement, ActionRefusal> {
        let actor = self.world.actor(mob_id)?;
        match self
            .world
            .place_check(&actor, pos, record, pay, &mut |cell, boxes| {
                self.placement_occupied_by_body(None, cell, boxes)
            }) {
            PlaceCheck::Ready(placement) => Ok(placement),
            PlaceCheck::Satisfied => Err(ActionRefusal::NothingToDo),
            PlaceCheck::Refused(refusal) => Err(refusal),
        }
    }
}
