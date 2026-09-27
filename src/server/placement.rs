use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::events::{BlockPlacePre, Outcome, PostEvent};
use crate::net::protocol::TargetRef;
use petramond_math::math::IVec3;
use petramond_world::block::{Aabb, Block, CellPart};

impl ServerGame {
    pub(super) fn place_held(
        &mut self,
        s: usize,
        target: Option<TargetRef>,
        predicted: bool,
        events: &mut TickEvents,
    ) -> Option<IVec3> {
        let stack = self.sessions[s].player.held().copied();
        let mut held = stack.and_then(|st| st.item.as_block());
        let pos = self.try_place(s, target, events)?;
        if stack.is_some_and(|st| !st.item.placement_variants().is_empty()) {
            held = Some(Block::from_id(
                self.world.data().chunk_block(pos.x, pos.y, pos.z),
            ));
        }
        events.player(s).placed_block = held;
        let hand = self.sessions[s].player.acting_hand;
        self.sessions[s].latch_swing(hand, mod_api::SwingKind::Place);
        if predicted {
            self.sessions[s].replication.presented_places.push(pos);
        }
        if let Some(block) = held {
            events.world.block_placed.push((pos, block));
            self.mods.emit(PostEvent::BlockPlaced {
                pos,
                block,
                player: Some(self.sessions[s].id),
            });
            self.push_block_noise(s, pos, crate::mob::NoiseKind::BlockPlaced);
        }
        Some(pos)
    }

    pub fn try_place(
        &mut self,
        s: usize,
        target: Option<TargetRef>,
        events: &mut TickEvents,
    ) -> Option<IVec3> {
        let h = target?;
        if h.normal == IVec3::ZERO {
            return None;
        }

        let stack = self.sessions[s].player.held()?;
        if stack.item.creative_only() && !self.sessions[s].player.abilities().restricted_items {
            return None;
        }
        let item = stack.item;
        let mut block = item.as_block().filter(|&b| b != Block::Air)?;

        // Right-click on replaceable blocks (short grass, fern...) places into the cell,
        // overwriting with no drop, not against the clicked face like normal. Air counts as
        // replaceable but never a raycast hit, so it's excluded. `p` feeds the torch support gate,
        // model footprint, and replaceable check alike.
        let player_facing =
            crate::rules::placement::facing_from_forward(self.sessions[s].player.forward());
        let inputs = crate::world::placement::PlaceInputs::of_click(
            self.world.data(),
            h.block,
            h.normal,
            h.spot_fraction(),
            player_facing,
            self.sessions[s].held_rotation_snapshot(),
            Some(item),
        );
        let p = inputs.place_pos;
        let variants = item.placement_variants();
        if !variants.is_empty() {
            let mut rng = petramond_worldgen::FeatureRng::positional(
                self.world.data().seed,
                0x706c_6163_656d_656e ^ self.world.current_tick(),
                p.x,
                p.y,
                p.z,
            );
            block = variants[rng.next_i32(0, variants.len() as i32 - 1) as usize];
        }
        let slab_stacks_in_hit = self
            .world
            .data()
            .slab_stack_slot_in_hit(
                block,
                h.block,
                self.sessions[s].held_slab_rotation(),
                h.normal,
                player_facing,
            )
            .is_some();
        let place_pos_for_pre = if slab_stacks_in_hit { h.block } else { p };

        {
            let mut pre = BlockPlacePre {
                pos: place_pos_for_pre,
                block,
                facing: player_facing,
                actor: crate::mob::EntityRef::Player(self.sessions[s].id),
            };
            let actor = Some(self.sessions[s].id);
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let bus = mods.bus_mut();
            if bus.block_place_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel {
                return None;
            }
        }

        if block.is_custom_shape() {
            if let Some(landed) = self.try_place_custom_shape(s, block, &inputs, events) {
                return landed;
            }
        }

        let plan = self
            .world
            .placement_plan(block, &inputs, &mut |cell, boxes| {
                self.placement_occupied_by_body(Some(s), cell, boxes)
            })?;
        self.touch_edit_cells(s, plan.cells());
        if !self.world.commit_placement(&plan, true) {
            return None;
        }
        self.carry_held_into(s, block, plan.anchor, plan.anchor_part());
        self.pay_for_placement(s);
        Some(plan.anchor)
    }

    fn carry_held_into(&mut self, s: usize, block: Block, anchor: IVec3, part: CellPart) {
        if let Some(held) = self.sessions[s].player.held().copied() {
            self.world.carry_into_cell(&held, block, anchor, part);
        }
    }

    fn pay_for_placement(&mut self, s: usize) {
        let player = &mut self.sessions[s].player;
        if !player.abilities().free_placement {
            let hand = player.acting_hand;
            player.inventory.decrement_held(hand);
        }
    }

    fn try_place_custom_shape(
        &mut self,
        s: usize,
        block: Block,
        inputs: &crate::world::placement::PlaceInputs,
        events: &mut TickEvents,
    ) -> Option<Option<IVec3>> {
        let shape_key = block.shape_kind().key();
        let shape_kind = block.shape_kind().0;
        let view = mod_api::PlaceInputsView {
            hit: inputs.hit.to_array(),
            normal: inputs.normal.to_array(),
            place_pos: inputs.place_pos.to_array(),
            player_facing: inputs.player_facing as u8,
        };
        let actor = Some(self.sessions[s].id);
        let result = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            mods.dispatch(world, sessions, actor, events, |host, ctx| {
                host.shape_placement_plan(ctx, shape_key, shape_kind, block.id(), view)
            })?
        };
        if !result.accepted {
            return Some(None);
        }
        let Some((anchor, write_block)) = crate::world::placement::validate_custom_plan(
            &result,
            block,
            shape_kind,
            inputs.place_pos,
        ) else {
            return Some(None);
        };
        // World-integrity gate the host always owns (the guest can see neither
        // gameplay bodies nor the save's replaceability): the cell must be LOADED
        // — `block_if_loaded`, never `chunk_block`, which collapses an unloaded
        // cell to replaceable air — and replaceable. `cur != write_block` is
        // redundant for the usual non-replaceable furniture (a replaceable cell
        // can't equal it) but guards the rare replaceable custom shape against a
        // no-op self-replace that would still burn a re-bake.
        match self
            .world
            .data()
            .block_if_loaded(anchor.x, anchor.y, anchor.z)
        {
            Some(cur) if cur.is_replaceable() && cur != write_block => {}
            _ => return Some(None),
        }
        if !self.world.data().placement_support_ok(write_block, anchor) {
            return Some(None);
        }
        let boxes = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let input = world.data().bake_cell_input(anchor, write_block);
            mods.dispatch(world, sessions, actor, events, |host, ctx| {
                host.bake_placement_sim_boxes(ctx, shape_key, shape_kind, input)
            })
        };
        let boxes: &[petramond_world::block::Aabb] = match &boxes {
            Some(b) => b,
            None => self
                .world
                .data()
                .custom_shape_boxes(anchor)
                .unwrap_or_else(|| write_block.collision_boxes()),
        };
        if self.placement_occupied_by_body(Some(s), anchor, boxes) {
            return Some(None);
        }
        let plan = crate::world::placement::PlacementPlan::single(
            anchor,
            write_block,
            petramond_world::block::ShapeState::NONE,
        );
        self.touch_edit_cells(s, plan.cells());
        if !self.world.commit_placement(&plan, true) {
            return Some(None);
        }
        self.carry_held_into(s, block, anchor, 0);
        self.pay_for_placement(s);
        Some(Some(anchor))
    }

    pub(super) fn placement_occupied_by_body(
        &self,
        placer: Option<usize>,
        cell: IVec3,
        boxes: &[Aabb],
    ) -> bool {
        use crate::rules::placement::{placement_blocked_by_bodies, player_occupies, Occupant};
        let players = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(i, sess)| {
                player_occupies(
                    Some(*i) == placer,
                    sess.player.health() > 0,
                    sess.player.is_spectator(),
                )
            })
            .map(|(_, sess)| Occupant::Player {
                feet: sess.player.pos,
            });
        let mobs = self
            .world
            .mobs()
            .instances()
            .iter()
            .filter(|m| !m.is_dead())
            .map(|m| Occupant::Mob {
                pos: m.pos,
                yaw: m.yaw,
                kind: m.kind,
            });
        placement_blocked_by_bodies(cell, boxes, players.chain(mobs))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn try_place_for_test(&mut self) -> bool {
        let target = self.sessions[0].input.look;
        self.try_place(0, target, &mut Default::default()).is_some()
    }
}
