use super::game::ServerGame;
use crate::block::{Aabb, Block, BlockInteraction, RenderShape};
use crate::block_state::StairState;
use crate::events::{BlockInteract, BlockPlacePre, Outcome, PostEvent};
use crate::facing::Facing;
use crate::game::tick::TickEvents;
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::TargetRef;
use crate::torch::TorchPlacement;

/// Hold-to-interact repeat cadence: a HELD use button re-runs the use-click
/// ladder this many ticks apart (250 ms at the 20 TPS tick).
pub(crate) const USE_REPEAT_TICKS: u32 = 5;

impl ServerGame {
    /// Placement / interaction, on the tick: consume a buffered secondary-button press
    /// once. Right-clicking a placed interactable block uses its block-owned capability
    /// rather than placing into the cell — unless sneaking, which falls through so the
    /// player can still build against it. An in-progress EAT (held button on a food
    /// item) advances every tick, click or not.
    pub(crate) fn tick_place(&mut self, s: usize, events: &mut TickEvents) {
        // The mob and block target under the crosshair at click time ride the
        // click; consume them with the press so a stale (or FRESHER — the
        // crosshair keeps moving while the click is queued) target can't
        // resolve the press somewhere the client's prediction isn't.
        let use_mob = std::mem::take(&mut self.sessions[s].pending_use_mob);
        let target = std::mem::take(&mut self.sessions[s].pending_place_target);
        if std::mem::take(&mut self.sessions[s].pending_place) {
            self.place_click(s, use_mob, target, events, false);
            // A real click paces the hold-repeat: the first repeat comes one
            // full interval after it (and spam clicks never compound rates).
            self.sessions[s].use_repeat_cooldown = USE_REPEAT_TICKS;
        } else {
            self.tick_use_repeat(s, events);
        }
        self.advance_eating(s, events);
    }

    /// A HELD use button re-runs the WHOLE use-click ladder every
    /// [`USE_REPEAT_TICKS`] against the current look latch, as if the player
    /// re-clicked: doors keep toggling, the hoe keeps tilling
    /// (`item_use_pre`), crops keep planting and harvesting, blocks keep
    /// placing. Two deliberate exceptions ride the `repeat` flag into
    /// [`place_click`](Self::place_click): a repeat never STARTS an eat (one
    /// eat per click — and no surprise bite when the look wanders off a door
    /// onto grass), and it ships no corrective cells (there is no client
    /// prediction to reconcile). A held button over nothing actionable
    /// attempts-and-does-nothing, exactly like today's single click; an
    /// in-progress eat owns the hold outright. Mob use (shears) does not
    /// repeat — a targeted mob blanks the block look, and the mob id only
    /// rides real clicks. Consumed repeats animate through the ordinary
    /// click machinery: `interacted`/`used_item` rows for observers, the
    /// `used_unpredicted` echo for the initiator's own jab (there was no
    /// client click to animate it).
    fn tick_use_repeat(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        if !sess.intent_use_held
            || !sess.intent_gameplay
            || sess.eating.is_some()
            || sess.player.is_spectator()
        {
            return;
        }
        sess.use_repeat_cooldown = sess.use_repeat_cooldown.saturating_sub(1);
        if sess.use_repeat_cooldown > 0 {
            return;
        }
        sess.use_repeat_cooldown = USE_REPEAT_TICKS;
        let look = sess.look;
        self.place_click(s, None, look, events, true);
    }

    /// Resolve one consumed secondary-button press against the CLICK's block
    /// target (never the look latch, which may be newer than the click).
    ///
    /// `repeat` marks a server-paced hold-repeat (see
    /// [`tick_use_repeat`](Self::tick_use_repeat)) rather than a real client
    /// click: the ladder runs identically except a repeat never STARTS an
    /// eat and never ships corrective cells — there is no click-side
    /// prediction to reconcile.
    fn place_click(
        &mut self,
        s: usize,
        use_mob: Option<u64>,
        target: Option<TargetRef>,
        events: &mut TickEvents,
        repeat: bool,
    ) {
        let request_id = self.sessions[s].pending_place_request_id.take();
        let predicted = std::mem::take(&mut self.sessions[s].pending_place_predicted);
        let jabbed = std::mem::take(&mut self.sessions[s].pending_place_jabbed);
        let mut consumed = false;
        let mut placed_at = None;
        // Using the held item ON the targeted mob (shears on a sheep) comes first:
        // while a mob is targeted `look` is None, so the block paths below
        // would no-op anyway.
        if self.try_shear_mob(s, use_mob) {
            events.player(s).used_item = true;
            consumed = true;
        } else {
            let interacted =
                !self.sessions[s].sneaking() && self.try_open_interactable(s, target, events);
            // The one place every consumed interaction passes through: the interact
            // hand jab defaults ON for all of them (see `GameEvents::interacted`).
            events.player(s).interacted |= interacted;
            // An item that is BOTH food and placeable (a carrot: edible produce
            // AND planting stock) tries its placement first — a VALID placement
            // wins over starting to eat; a refused one (unplaceable target,
            // cancelled block_place_pre) falls through to the ordinary eat.
            // Ordering the real placement attempt ahead of the eat gate keeps
            // one dispatch per event: no dry-run duplicating `try_place`.
            let contextual_place = !interacted
                && self.sessions[s]
                    .player
                    .inventory
                    .selected()
                    .is_some_and(|st| {
                        st.item.food().is_some()
                            && st.item.as_block().is_some_and(|b| b != Block::Air)
                    });
            if contextual_place {
                placed_at = self.place_held(s, target, predicted, events);
            }
            if interacted || placed_at.is_some() || (!repeat && self.try_start_eating(s, events)) {
                consumed = true;
            } else if self.try_use_item(s, target, events) {
                events.player(s).used_item = true;
                consumed = true;
            } else if !contextual_place {
                placed_at = self.place_held(s, target, predicted, events);
                consumed = placed_at.is_some();
            }
        }
        // A consumed click whose initiator stayed silent (its replica could
        // not foresee the effect — a mod-cancelled use/interact like tilling
        // or a right-click harvest) gets its hand jab echoed back; `jabbed`
        // guarantees this can never double an already-played one.
        if consumed && !jabbed {
            events.player(s).used_unpredicted = true;
        }
        // The client's ghost convention is `target.block + normal` — accept
        // ONLY a placement that landed exactly there (accept never rolls
        // back, so anything else must DENY to clear the ghost: a click
        // consumed by an interact/eat/use, a replace-in-place, a slab stack
        // into the hit cell, nothing at all).
        let predicted = target
            .filter(|t| t.normal != IVec3::ZERO)
            .map(|t| t.block + t.normal);
        let accepted = placed_at.is_some() && placed_at == predicted;
        if let Some(id) = request_id {
            self.push_action_outcome(
                s,
                id,
                accepted,
                (!accepted).then_some(crate::net::protocol::ActionDenyReason::Denied),
            );
        }
        // Reconcile channel: when the click did nothing (the client may have
        // clicked a block that only exists in ITS replica) or its prediction
        // was denied, ship the authoritative state of the disputed cells.
        // Repeats skip it — no click, no prediction, nothing to reconcile
        // (a held button over a no-op target must not stream deltas).
        if let Some(t) = target.filter(|_| !repeat) {
            let disputed = !consumed || (request_id.is_some() && !accepted);
            if disputed {
                let sess = &mut self.sessions[s];
                sess.pending_corrective_cells.push(t.block);
                if t.normal != IVec3::ZERO {
                    sess.pending_corrective_cells.push(t.block + t.normal);
                }
            }
        }
    }

    /// Ordinary placement of the held item's block, with the shared
    /// bookkeeping every placement path owes: the place-sound latch, the
    /// initiator's presented-place strip, and the placed events.
    fn place_held(
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
        }
        Some(pos)
    }

    /// If the click's block target has a secondary-use capability, apply it
    /// and return `true` (consuming the right-click).
    fn try_open_interactable(
        &mut self,
        s: usize,
        target: Option<TargetRef>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(h) = target else {
            return false;
        };
        let block = Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
        // A handler cancelling `block_interact` consumed the click (this is how mod
        // blocks will open their own GUIs); the block's built-in capability is skipped.
        let mut pre = BlockInteract {
            pos: h.block,
            block,
        };
        let cancelled = {
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            let sess = &mut sessions[s];
            bus.block_interact(
                world,
                &mut sess.player,
                &mut sess.gui_state,
                events,
                &mut pre,
            ) == Outcome::Cancel
        };
        if cancelled {
            return true;
        }
        // Menu opens join the ordered menu-action stream. Placement resolves
        // before the Menu stage, so this appends behind any close/click/craft
        // messages already received for the old screen.
        match block.interaction() {
            BlockInteraction::OpenCraftingTable => {
                self.sessions[s]
                    .pending_menu_actions
                    .push(crate::server::player::PendingMenuAction::OpenCraftingTable);
                true
            }
            BlockInteraction::OpenFurnace => {
                self.sessions[s].pending_menu_actions.push(
                    crate::server::player::PendingMenuAction::OpenFurnace(h.block),
                );
                true
            }
            BlockInteraction::OpenChest => {
                self.sessions[s]
                    .pending_menu_actions
                    .push(crate::server::player::PendingMenuAction::OpenChest(h.block));
                true
            }
            BlockInteraction::OpenFurnitureWorkbench => {
                self.sessions[s]
                    .pending_menu_actions
                    .push(crate::server::player::PendingMenuAction::OpenWorkbench);
                true
            }
            BlockInteraction::OpenModGui(kind) => {
                // The clicked block's position rides the session so gui_click
                // dispatches carry where the GUI was opened from.
                self.sessions[s].pending_menu_actions.push(
                    crate::server::player::PendingMenuAction::OpenModGui {
                        kind,
                        pos: Some(h.block),
                    },
                );
                true
            }
            // Right-clicking a door toggles it: the open/closed bit flips on this tick
            // (so collision updates at once and the player can step through), and the
            // visual swing is eased from the door's current angle. Seed the swing entry
            // BEFORE the toggle so it starts from the old pose, then eases to the new one.
            BlockInteraction::ToggleDoor => {
                if let Some(lower) = self.world.door_lower_cell(h.block.x, h.block.y, h.block.z) {
                    self.world.toggle_door(h.block);
                    // The new open state after the toggle — drives open vs close sound.
                    let now_open = self
                        .world
                        .door_state_at(lower.x, lower.y, lower.z)
                        .map(|st| st.open)
                        .unwrap_or(true);
                    // The swing animation + positional sound come from this
                    // event client-side (`apply_world_effects` / the app),
                    // like any observer's will.
                    events.world.door_changed.push((lower, now_open));
                    // The TOGGLER's own one-shot (hand flick).
                    events.player(s).toggled_door = Some(now_open);
                }
                true
            }
            // Right-clicking a bed sets the spawn point beside it and starts
            // the sleep (see `game::bed`); the app opens the sleep overlay via
            // the open request this queues.
            BlockInteraction::Sleep => {
                events.player(s).bed_interacted = true;
                self.start_sleep(s, h.block);
                true
            }
            BlockInteraction::None => false,
        }
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
        let slab_stack_slot = (block.render_shape() == RenderShape::Slab
            && crate::slab::is_slab(looked_at))
        .then(|| {
            crate::slab::stack_slot(
                self.sessions[s].held_slab_rotation(),
                h.normal,
                player_facing,
            )
        })
        .flatten();
        let slab_stacks_in_hit = slab_stack_slot.is_some_and(|slot| {
            crate::slab::can_add_layer(
                self.world.slab_state_at(h.block.x, h.block.y, h.block.z),
                slot,
            )
        });
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

        // A torch only mounts on a floor or wall (never a ceiling) and needs a usable
        // support face. Resolve that up front so an invalid spot is a no-op (the click
        // neither places nor consumes the torch) rather than leaving a floating one.
        // When REPLACING a plant the torch always drops to the FLOOR of that cell — so
        // right-clicking grass from any angle, even its side, stands a floor torch where
        // the grass was instead of failing on the side face's would-be wall mount.
        let torch_placement = if block == Block::Torch {
            let tp = if replacing_in_place {
                TorchPlacement::Floor
            } else {
                TorchPlacement::from_place_normal(h.normal)?
            };
            if !self.world.torch_supported_at(p, tp) {
                return None;
            }
            Some(tp)
        } else {
            None
        };

        if block.render_shape() == RenderShape::Slab {
            let (target, slot) = match slab_stack_slot {
                Some(slot) if slab_stacks_in_hit => (h.block, slot),
                _ => (
                    p,
                    crate::slab::slot_for_rotation(
                        self.sessions[s].held_slab_rotation(),
                        h.normal,
                        player_facing,
                    ),
                ),
            };
            let target_block = Block::from_id(self.world.chunk_block(target.x, target.y, target.z));
            if !crate::slab::is_slab(target_block) && !self.world.placement_cell_open(target) {
                return None;
            }
            let next_state = self.world.slab_layer_target_state(target, block, slot)?;
            let boxes = crate::slab::boxes_for_state(next_state);
            let blocked = self.placement_occupied_by_body(s, target, boxes);
            if !blocked && self.world.place_slab_layer(target, block, slot) {
                self.sessions[s].player.inventory.decrement_selected();
                return Some(target);
            }
            return None;
        }

        // A bbmodel block places its WHOLE footprint (the workbench is 2×2×1): every
        // occupied cell must be loaded + replaceable AND clear of blocking bodies, or the
        // placement fails as a unit (nothing placed, the held item kept). Multi-cell
        // models, and models marked directionalView, are oriented from the player's
        // facing through the model's own placement orientation (the workbench spans
        // left-to-right across the view, the bed runs front-to-back away from it);
        // `p` is the front-left bottom anchor from the player's view.
        if let RenderShape::Model(kind) = block.render_shape() {
            let multi_cell = crate::block_model::instance(kind).cells.len() > 1;
            let facing = if block.directional_view() || multi_cell {
                crate::block_model::def(kind)
                    .orientation
                    .apply(player_facing)
            } else {
                crate::block_model::DEFAULT_MODEL_FACING
            };
            let base = if block.directional_view() || multi_cell {
                crate::block_model::base_from_front_left_anchor(p, kind, facing)
            } else {
                p
            };
            if !self.world.model_footprint_clear_facing(base, kind, facing) {
                return None;
            }
            let blocked = crate::block_model::oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .any(|(c, off)| {
                    self.placement_occupied_by_body(
                        s,
                        c,
                        crate::block_model::collision_boxes_oriented(kind, off, facing),
                    )
                });
            if !blocked && self.world.place_model_block_facing(base, block, facing) {
                self.sessions[s].player.inventory.decrement_selected();
                return Some(base);
            }
            return None;
        }

        // A door is a 2-tall thin block: its lower cell is `p`, the upper is the cell
        // above. Both must be loaded + replaceable AND give it a floor to stand on
        // (`door_footprint_clear`), and the closed slab must not trap the player or a
        // mob. It sits on the edge nearest the placer (the player's facing). Placement
        // + the paired door state live in `World::place_door`.
        if block.render_shape() == RenderShape::Door {
            let facing = player_facing;
            let upper = p + IVec3::new(0, 1, 0);
            if !self.world.door_footprint_clear(p) {
                return None;
            }
            let closed = |top: bool| {
                crate::door::collision_boxes(crate::door::DoorState {
                    facing,
                    open: false,
                    top,
                })
            };
            let blocked = [(p, false), (upper, true)]
                .into_iter()
                .any(|(c, top)| self.placement_occupied_by_body(s, c, closed(top)));
            if !blocked && self.world.place_door(p, block, facing) {
                self.sessions[s].player.inventory.decrement_selected();
                return Some(p);
            }
            return None;
        }

        if block.render_shape() == RenderShape::Stair {
            let facing = player_facing;
            let state = StairState::new(facing, self.sessions[s].held_stair_half());
            if !self.world.placement_cell_open(p) {
                return None;
            }
            let boxes = self.world.resolved_stair_boxes(p, state);
            let blocked = self.placement_occupied_by_body(s, p, boxes);
            if !blocked && self.world.place_stair(p, block, state) {
                self.sessions[s].player.inventory.decrement_selected();
                return Some(p);
            }
            return None;
        }

        // A pane occupies only its resolved post + arms, so the overlap gate tests
        // those thin boxes (a player standing beside the centre line doesn't block
        // it the way a full cube would). No stored state: the connections are
        // re-resolved from neighbours wherever the shape is read.
        if block.render_shape() == RenderShape::Pane {
            if !self.world.placement_cell_open(p) {
                return None;
            }
            let boxes = self.world.pane_boxes_at(p);
            let blocked = self.placement_occupied_by_body(s, p, boxes);
            if !blocked && self.world.set_block_world(p.x, p.y, p.z, block) {
                self.sessions[s].player.inventory.decrement_selected();
                return Some(p);
            }
            return None;
        }

        // Substrate gate: a block that roots in a particular ground — a flower in soil, a
        // cactus in sand, a mushroom on soil or stone — places only when the cell directly
        // below is a ground it accepts (`can_root_on`). Blocks with no such rule (almost
        // all of them) accept anything; a torch is gated by its own opaque-face check
        // above. Staying put once placed is the separate job of the FRAGILE behaviour.
        let below = self.world.physics_block(p.x, p.y - 1, p.z);
        if !block.can_root_on(below) {
            return None;
        }

        // A block with no collision box (a torch, grass, a fern, …) traps nothing, so it
        // may be placed inside an entity; a block that WOULD collide cannot be placed
        // where its placed shape overlaps a gameplay body.
        let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
        let clear_of_bodies = !self.placement_occupied_by_body(s, p, block.collision_boxes());
        if target.is_replaceable()
            && clear_of_bodies
            && if block.is_log() {
                let axis = self.sessions[s].held_log_axis_for_facing(player_facing);
                self.world.place_log(p, block, axis)
            } else {
                self.world.set_block_world(p.x, p.y, p.z, block)
            }
        {
            // A placed furnace/chest gets an empty block-entity from the moment it
            // exists. Blocks marked directionalView have their front oriented to face
            // the player; a torch records how it is mounted (floor vs which wall) for
            // the mesher + outline.
            let placed_facing = if block.directional_view() {
                player_facing
            } else {
                crate::block_model::DEFAULT_MODEL_FACING
            };
            if block == Block::Furnace {
                self.world.insert_furnace(p, placed_facing);
            } else if block == Block::Chest {
                self.world.insert_chest(p, placed_facing);
            } else if let Some(tp) = torch_placement {
                self.world.insert_torch(p, tp);
            }
            self.sessions[s].player.inventory.decrement_selected();
            Some(p)
        } else {
            None
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

    /// Test-only: latch a use click on session `s` aimed at its current look —
    /// what a real client click ships as its click-time target.
    #[cfg(test)]
    pub(crate) fn queue_place_click_for_test(&mut self, s: usize) {
        let sess = &mut self.sessions[s];
        sess.pending_place_target = sess.look;
        sess.pending_place = true;
        // The hook models a client that ran its full place prediction (the
        // common case), so the echo strip applies like production.
        sess.pending_place_predicted = true;
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
