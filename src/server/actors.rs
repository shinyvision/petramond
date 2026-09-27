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
        self.mods.emit(PostEvent::ActorActed {
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

    pub(super) fn apply_actor_interact(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        events: &mut TickEvents,
    ) {
        let refusal = self.actor_use(mob_id, pos, events).err();
        self.mods.emit(PostEvent::ActorActed {
            actor: EntityRef::Mob(mob_id),
            pos,
            action: ActorAction::Use,
            refusal,
        });
    }

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
            petramond_world::block::BlockInteraction::ToggleTrapdoor => self
                .swing_trapdoor(pos, events)
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
        self.mods.emit(PostEvent::ActorActed {
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
                mods,
                ..
            } = self;
            let bus = mods.bus_mut();
            bus.block_place_pre(world, sessions, None, events, &mut pre) == Outcome::Cancel
        };
        if cancelled {
            return Err(ActionRefusal::Vetoed);
        }
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
            let actor = self.world.actor(mob_id)?;
            let paid = self
                .world
                .mobs_mut()
                .container_mut(actor.id)
                .is_some_and(|c| c.take_all(std::slice::from_ref(&paid)));
            debug_assert!(paid, "the carried cost was proven just before the commit");
        }
        events.world.block_placed.push((writes.anchor, block));
        self.mods.emit(PostEvent::BlockPlaced {
            pos: writes.anchor,
            block,
            player: None,
        });
        self.push_noise_from(
            EntityRef::Mob(mob_id),
            writes.anchor,
            crate::mob::NoiseKind::BlockPlaced,
        );
        Ok(())
    }

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
