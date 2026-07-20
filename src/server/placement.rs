use super::game::ServerGame;
use crate::block::{Aabb, Block, BlockInteraction};
use crate::events::{BlockInteract, BlockPlacePre, Outcome, PostEvent};
use crate::facing::Facing;
use crate::game::tick::TickEvents;
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::TargetRef;
use crate::server::player::PendingUseClick;

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
        // Taking the whole click clears its target, request, presentation
        // verdict, and held-selection guard together. A newer hotbar
        // selection invalidates the attempt before any rung can observe or
        // mutate through a different item than receipt-time targeting used.
        if let Some(click) = self.sessions[s].pending_use_click.take() {
            if click.selection_still_matches(&self.sessions[s].player) {
                self.place_click(s, click, events, false);
            } else {
                self.reject_selection_changed_use_click(s, click);
            }
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
        let click = PendingUseClick::capture(&sess.player, None, look, None, false, false);
        self.place_click(s, click, events, true);
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
        click: PendingUseClick,
        events: &mut TickEvents,
        repeat: bool,
    ) {
        let PendingUseClick {
            mob: use_mob,
            target,
            request_id,
            predicted,
            jabbed,
            ..
        } = click;
        let mut consumed = false;
        let mut placed_at = None;
        // A targeted mob resolves first: mods get the interaction before any
        // engine mob use (`mob_interact`, cancel = consumed — how a mod makes
        // a mob mountable/tradeable), then shears. While a mob is targeted
        // `look` is None, so the block paths below would no-op anyway.
        if self.mob_interact(s, use_mob, events) {
            events.player(s).interacted = true;
            consumed = true;
        } else if self.try_shear_mob(s, use_mob) {
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
                self.queue_use_corrective_cells(s, t);
            }
        }
    }

    /// Refuse a real click whose selected slot/item changed after receipt.
    /// This is the same no-op contract as any other denied use: answer its
    /// prediction request and correct the disputed cells, but dispatch no
    /// gameplay rung and emit no action presentation.
    fn reject_selection_changed_use_click(&mut self, s: usize, click: PendingUseClick) {
        if let Some(id) = click.request_id {
            self.push_action_outcome(
                s,
                id,
                false,
                Some(crate::net::protocol::ActionDenyReason::Denied),
            );
        }
        if let Some(target) = click.target {
            self.queue_use_corrective_cells(s, target);
        }
    }

    fn queue_use_corrective_cells(&mut self, s: usize, target: TargetRef) {
        let sess = &mut self.sessions[s];
        sess.pending_corrective_cells.push(target.block);
        if target.normal != IVec3::ZERO {
            sess.pending_corrective_cells
                .push(target.block + target.normal);
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
            self.push_block_noise(s, pos, crate::mob::NoiseKind::BlockPlaced);
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
            item: self.sessions[s]
                .player
                .inventory
                .selected()
                .map(|st| st.item),
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
            // Engine containers and mod GUIs ride ONE open lane. The clicked
            // block's position rides the session so per-kind session setup
            // (chest viewer, machine gauges, mod container anchoring) and
            // gui_click dispatches know where the GUI was opened from.
            BlockInteraction::OpenGui(kind) => {
                self.sessions[s].pending_menu_actions.push(
                    crate::server::player::PendingMenuAction::OpenGui {
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
            stair_half: self.sessions[s].held_stair_half(),
            slab_rotation: self.sessions[s].held_slab_rotation(),
            log_axis: self.sessions[s].held_log_axis_for_facing(player_facing),
        };
        let plan = self
            .world
            .placement_plan(block, &inputs, &mut |cell, boxes| {
                self.placement_occupied_by_body(s, cell, boxes)
            })?;
        if !self.world.commit_placement(block, &plan, true) {
            return None;
        }
        self.sessions[s].player.inventory.decrement_selected();
        Some(plan.anchor)
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
        // The hook models a client that ran its full place prediction (the
        // common case), so the echo strip applies like production.
        sess.pending_use_click = Some(PendingUseClick::capture(
            &sess.player,
            None,
            sess.look,
            None,
            true,
            false,
        ));
    }

    /// Test-only variant of [`queue_place_click_for_test`] carrying a claimed
    /// stable mob id instead of a block target.
    #[cfg(test)]
    pub(crate) fn queue_mob_use_click_for_test(&mut self, s: usize, mob: u64) {
        let sess = &mut self.sessions[s];
        sess.pending_use_click = Some(PendingUseClick::capture(
            &sess.player,
            Some(mob),
            None,
            None,
            true,
            false,
        ));
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
