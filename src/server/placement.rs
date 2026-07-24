//! Block placement — the placement consumer of the interact dispatch (see
//! `server::interact`): validity through the SHARED per-shape placement
//! ladder (`World::placement_plan`), the commit, and the bookkeeping every
//! placement path owes.

use super::game::ServerGame;
use crate::block::{Aabb, Block, ShapeFamily};
use crate::events::{BlockPlacePre, Outcome, PostEvent, SimCtx};
use crate::facing::Facing;
use crate::game::tick::TickEvents;
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::TargetRef;

impl ServerGame {
    /// Ordinary placement of the held item's block, with the shared
    /// bookkeeping every placement path owes: the place-sound latch, the
    /// initiator's presented-place strip, and the placed events.
    pub(super) fn place_held(
        &mut self,
        s: usize,
        target: Option<TargetRef>,
        predicted: bool,
        events: &mut TickEvents,
    ) -> Option<IVec3> {
        // Capture the held block before `try_place` consumes it: on success that is
        // exactly the block placed, which the client maps to a place sound.
        let held = self.sessions[s]
            .player
            .inventory
            .selected()
            .and_then(|st| st.item.as_block());
        let pos = self.try_place(s, target, events)?;
        events.player(s).placed_block = held;
        // Strip this cell from the initiator's TickUpdate.events
        // only when they PRESENTED the place locally (full ghost).
        // An unpredicted placement — oriented model, replace-in-
        // place, slab stack, frozen ledger — keeps its event, or
        // the initiator never hears their own place.
        if predicted {
            self.sessions[s].presented_places.push(pos);
        }
        if let Some(block) = held {
            // Every observer presents the placement (positional sound)
            // from the world-anchored event.
            events.world.block_placed.push((pos, block));
            self.bus.emit(PostEvent::BlockPlaced { pos, block });
            self.push_block_noise(s, pos, crate::mob::NoiseKind::BlockPlaced);
        }
        Some(pos)
    }

    /// Attempt to place the held block against the click's target face;
    /// returns the anchor cell it landed in (the front-left-bottom cell for
    /// multi-cell models, the lower cell for doors), or `None` if nothing was
    /// placed.
    pub(crate) fn try_place(
        &mut self,
        s: usize,
        target: Option<TargetRef>,
        events: &mut TickEvents,
    ) -> Option<IVec3> {
        let h = target?;
        if h.normal == IVec3::ZERO {
            return None;
        }

        let block = match self.sessions[s].player.inventory.selected() {
            Some(stack) => match stack.item.as_block() {
                Some(b) if b != Block::Air => b,
                _ => return None,
            },
            None => return None,
        };

        // Right-clicking a replaceable block (short grass, a fern…) while holding a block
        // places straight INTO its cell, overwriting it with no drop — the block just
        // disappears, as if the cell were empty. Otherwise the placement builds against
        // the clicked face. Air is replaceable too (a placement may overwrite it) but is
        // never itself a raycast hit, so exclude it. `p` then feeds the torch support
        // gate, the model footprint, and the final replaceable check uniformly.
        let looked_at = Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
        let replacing_in_place = looked_at.is_replaceable() && looked_at != Block::Air;
        let p = if replacing_in_place {
            h.block
        } else {
            h.block + h.normal
        };
        let player_facing = facing_from_forward(self.sessions[s].player.forward());
        let slab_stacks_in_hit = self
            .world
            .slab_stack_slot_in_hit(
                block,
                h.block,
                self.sessions[s].held_slab_rotation(),
                h.normal,
                player_facing,
            )
            .is_some();
        let place_pos_for_pre = if slab_stacks_in_hit { h.block } else { p };

        // The placement decision, announced before the shape-specific validity
        // checks: a cancelled `block_place_pre` refuses the placement outright (the
        // click does nothing and the held item is kept). `facing` is the raw
        // player-derived placement input; the shape paths below may orient it further.
        {
            let mut pre = BlockPlacePre {
                pos: place_pos_for_pre,
                block,
                facing: player_facing,
            };
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            let sess = &mut sessions[s];
            if bus.block_place_pre(
                world,
                &mut sess.player,
                &mut sess.gui_state,
                events,
                &mut pre,
            ) == Outcome::Cancel
            {
                return None;
            }
        }

        // The per-shape ladder is the SHARED placement rule (also evaluated by
        // the client place ghost against its replica): validity + the exact
        // state write. Only the body-occupancy answer is authoritative-side
        // specific.
        let inputs = crate::world::placement::PlaceInputs {
            hit: h.block,
            normal: h.normal,
            place_pos: p,
            replacing_in_place,
            player_facing,
            held_rotation: self.sessions[s].held_rotation_snapshot(),
            held: self.sessions[s].selected_item(),
        };
        // A Layer-3 custom shape places through its OWN pack's WASM callback
        // (footprint + orientation + initial state are the shape's to decide),
        // not the engine ladder. A pack with no reachable owner (disabled /
        // trapped) falls through to the ordinary ladder so its cells still
        // place as a plain cube — the block id stays load-bearing.
        if block.shape_family() == ShapeFamily::Custom {
            if let Some(landed) = self.try_place_custom_shape(s, block, &inputs, events) {
                return landed;
            }
        }

        let plan = self
            .world
            .placement_plan(block, &inputs, &mut |cell, boxes| {
                self.placement_occupied_by_body(s, cell, boxes)
            })?;
        if !self.world.commit_placement(&plan, true) {
            return None;
        }
        self.restore_carry(s, block, plan.anchor);
        self.sessions[s].player.inventory.decrement_selected();
        Some(plan.anchor)
    }

    /// Place a Layer-3 custom shape via its pack's `shape_placement_plan`
    /// callback. `None` = no reachable owner, so `try_place` falls through to
    /// the engine ladder; `Some(None)` = the callback refused (or nothing
    /// placeable); `Some(Some(anchor))` = placed at `anchor`. The client never
    /// ghosts a pack block, so this authoritative decision arrives unpredicted.
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
        let result = {
            let Self {
                world,
                sessions,
                bus,
                mods,
                ..
            } = self;
            let sess = &mut sessions[s];
            let mut ctx = SimCtx {
                world,
                player: &mut sess.player,
                gui_state: &mut sess.gui_state,
                feed: events,
                queue: bus.queue_mut(),
            };
            mods.shape_placement_plan(&mut ctx, shape_key, shape_kind, block.id(), view)?
        };
        if !result.accepted {
            return Some(None);
        }
        // The SHARED plan validation (single-cell, click radius, the
        // sibling-row override gate) — the client place ghost runs the same
        // rule against its replica, so both sides compute the same write by
        // construction.
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
        match self.world.block_if_loaded(anchor.x, anchor.y, anchor.z) {
            Some(cur) if cur.is_replaceable() && cur != write_block => {}
            _ => return Some(None),
        }
        // The body-occupancy gate every engine placement path runs: a custom
        // shape's solid boxes may not trap a player or mob. The bake cache
        // only holds PLACED cells, so for this not-yet-placed cell the shape's
        // own sim bake answers the hypothetical (a pure function of the same
        // cell input the post-placement pump will see) — falling back to a
        // cached bake, then the row's static collision, when no owner replies.
        let boxes = {
            let Self {
                world,
                sessions,
                bus,
                mods,
                ..
            } = self;
            // The hypothetical cell's bake input — the SAME builder the
            // post-placement pump uses, so the gate and the pump bake from
            // identical inputs by construction.
            let input = world.bake_cell_input(anchor, write_block);
            let sess = &mut sessions[s];
            let mut ctx = SimCtx {
                world,
                player: &mut sess.player,
                gui_state: &mut sess.gui_state,
                feed: events,
                queue: bus.queue_mut(),
            };
            mods.bake_placement_sim_boxes(&mut ctx, shape_key, shape_kind, input)
        };
        let boxes: &[crate::block::Aabb] = match &boxes {
            Some(b) => b,
            None => self
                .world
                .custom_shape_boxes(anchor)
                .unwrap_or_else(|| write_block.collision_boxes()),
        };
        if self.placement_occupied_by_body(s, anchor, boxes) {
            return Some(None);
        }
        // A stateless single-cell write of the plan's validated row (held or
        // the sibling-row orientation override) — the same generic commit
        // every family uses.
        let plan = crate::world::placement::PlacementPlan::single(
            anchor,
            write_block,
            crate::block::ShapeState::NONE,
        );
        if !self.world.commit_placement(&plan, true) {
            return Some(None);
        }
        self.restore_carry(s, block, anchor);
        self.sessions[s].player.inventory.decrement_selected();
        Some(Some(anchor))
    }

    /// Carry courier (place side): copy the held stack's instance-data
    /// entries listed in the placed row's `petramond:carry` back into cell KV
    /// at the anchor. Runs AFTER the commit (a block write wipes the cell's
    /// KV) and BEFORE `decrement_selected` (the held stack still carries the
    /// variant). A non-writable section refuses silently — the block stands,
    /// its carried data is lost, same as any racing KV write.
    fn restore_carry(&mut self, s: usize, block: Block, anchor: IVec3) {
        let carry = block.carry();
        if carry.is_empty() {
            return;
        }
        let Some(held) = self.sessions[s].player.inventory.selected() else {
            return;
        };
        let Some(map) = crate::item::variant::get(held.variant) else {
            return;
        };
        for &key in carry {
            if let Some(v) = map.get(key) {
                self.world
                    .cell_kv_set(anchor.x, anchor.y, anchor.z, key.to_owned(), v.clone());
            }
        }
    }

    /// Whether the placed collision boxes at `cell` overlap a gameplay body that
    /// blocks placement. The acting player always counts, preserving the
    /// self-trapping guard. Other sessions count while alive and non-spectator;
    /// sleeping players still count because sleep keeps the gameplay body on
    /// the mattress. Dead mobs do not count, matching the ragdoll rule.
    fn placement_occupied_by_body(&self, actor: usize, cell: IVec3, boxes: &[Aabb]) -> bool {
        self.sessions.iter().enumerate().any(|(i, sess)| {
            (i == actor || (sess.player.health() > 0 && !sess.player.is_spectator()))
                && sess.player.body().overlaps_block_boxes(cell, boxes)
        }) || self.world.mobs().any_overlapping_boxes(cell, boxes)
    }

    /// Test-only wrapper keeping the old bool-shaped call for placement tests
    /// (the latched look stands in for the click target they never build).
    #[cfg(test)]
    pub(crate) fn try_place_for_test(&mut self) -> bool {
        let target = self.sessions[0].look;
        self.try_place(0, target, &mut Default::default()).is_some()
    }
}

/// The furnace facing for a block placed while looking along `forward`: the front
/// (mouth) points back toward the player — opposite the camera's horizontal look
/// direction — snapped to the nearest cardinal.
pub(crate) fn facing_from_forward(forward: Vec3) -> Facing {
    let (fx, fz) = (-forward.x, -forward.z);
    if fx.abs() >= fz.abs() {
        if fx >= 0.0 {
            Facing::East
        } else {
            Facing::West
        }
    } else if fz >= 0.0 {
        Facing::South
    } else {
        Facing::North
    }
}
