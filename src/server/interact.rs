//! The interact dispatch: one use click (or hold-repeat) becomes ONE
//! [`InteractAttempt`] — the most primitive gesture possible: what the
//! crosshair held (a block cell + face, a live mob) and who is acting.
//! Nothing else rides the attempt; every gate (sneak, held item, block
//! identity) belongs to the CONSUMER that cares about it, read from the
//! actor's state, never pre-interpreted by the dispatcher.
//!
//! The attempt walks [`CONSUMERS`] — an ordered registry, not an if-ladder.
//! Each consumer inspects the attempt and either claims it or passes; the
//! first claim wins and nothing later runs. Mods participate through the
//! `interact_attempt` bus event (one consumer entry dispatches it; a
//! handler's Cancel is a claim); engine capabilities are sibling entries in
//! the same registry. Whether the hand jabs is exactly whether ANYTHING
//! claimed the attempt (`GameEvents::interacted` / `used_item` at the
//! claiming consumer, plus the `used_unpredicted` echo for effects the
//! initiator's replica could not foresee).

use super::game::ServerGame;
use crate::block::{Block, BlockInteraction};
use crate::events::{InteractAttempt, Outcome};
use crate::game::tick::TickEvents;
use crate::mathh::IVec3;
use crate::net::protocol::TargetRef;
use crate::server::player::PendingUseClick;

/// Hold-to-interact repeat cadence: a HELD use button re-runs the interact
/// dispatch this many ticks apart (250 ms at the 20 TPS tick).
pub(crate) const USE_REPEAT_TICKS: u32 = 5;

/// Click plumbing that rides beside the attempt but is not part of the
/// gesture: the raw click target (the placement consumers' input), the
/// client's prediction claims, and the hold-repeat flag. Consumers read it;
/// the attempt payload the mods see never carries it.
pub(crate) struct ClickMeta {
    /// The claimed block target (click-time latch, reach-validated).
    pub target: Option<TargetRef>,
    /// Whether the client ran a full place ghost for this click.
    pub predicted: bool,
    /// A server-paced hold-repeat rather than a real client click: never
    /// STARTS an eat, ships no corrective cells.
    pub repeat: bool,
}

/// One consumer's verdict on the attempt.
enum Claim {
    /// Not this consumer's business — the walk continues.
    Pass,
    /// The attempt is consumed; the walk ends.
    Claimed,
    /// Consumed by a placement that landed its anchor at this cell (the
    /// ghost-accept convention needs the exact anchor).
    Placed(IVec3),
}

/// The consumer registry, in claim order. Deterministic and data-shaped: a
/// new engine capability is a new entry here (plus its client prediction
/// rule), never a branch in the dispatcher.
const CONSUMERS: &[fn(&mut ServerGame, usize, &InteractAttempt, &ClickMeta, &mut TickEvents) -> Claim] = &[
    // Mods first: every attempt, sneak or not, block or mob — a handler's
    // Cancel is a claim (mod GUIs, boat boarding, the trough take-out).
    ServerGame::consume_mod_attempt,
    // Engine mob use: shears on a shearable mob.
    ServerGame::consume_shear,
    // The block's built-in capability (GUI open, door, bed) — passes on
    // sneak via the shared claim rule the client predictions also run.
    ServerGame::consume_builtin_block,
    // A dual-natured held item (food AND placeable — a plantable carrot)
    // tries its placement before the eat gate: a VALID placement wins over
    // starting to eat; a refused one passes so the eat still sees the click.
    // Ordering the real attempt ahead of the eat keeps one dispatch per
    // event: no dry-run duplicating `try_place`.
    ServerGame::consume_contextual_place,
    // Eating the held food (never started by a hold-repeat).
    ServerGame::consume_eat,
    // The held item's own use (`item_use_pre`, then the engine buckets).
    ServerGame::consume_item_use,
    // Ordinary placement of the held block (skipped for dual-natured items —
    // their placement already ran above).
    ServerGame::consume_place,
];

impl ServerGame {
    /// Interact / placement, on the tick: consume a buffered secondary-button
    /// press once and dispatch it down the consumer registry. An in-progress
    /// EAT (held button on a food item) advances every tick, click or not.
    pub(crate) fn tick_place(&mut self, s: usize, events: &mut TickEvents) {
        // Taking the whole click clears its target, request, presentation
        // verdict, and held-selection guard together. A newer hotbar
        // selection invalidates the attempt before any consumer can observe
        // or mutate through a different item than receipt-time targeting used.
        if let Some(click) = self.sessions[s].pending_use_click.take() {
            if click.selection_still_matches(&self.sessions[s].player) {
                self.dispatch_use_click(s, click, events, false);
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

    /// A HELD use button re-runs the WHOLE interact dispatch every
    /// [`USE_REPEAT_TICKS`] against the current look latch, as if the player
    /// re-clicked: doors keep toggling, the hoe keeps tilling
    /// (`item_use_pre`), crops keep planting and harvesting, blocks keep
    /// placing. Two deliberate exceptions ride [`ClickMeta::repeat`]: a
    /// repeat never STARTS an eat (one eat per click — and no surprise bite
    /// when the look wanders off a door onto grass), and it ships no
    /// corrective cells (there is no client prediction to reconcile). A held
    /// button over nothing actionable attempts-and-does-nothing, exactly
    /// like a single click; an in-progress eat owns the hold outright. Mob
    /// use does not repeat — a targeted mob blanks the block look, and the
    /// mob id only rides real clicks. Consumed repeats animate through the
    /// ordinary click machinery: `interacted`/`used_item` rows for
    /// observers, the `used_unpredicted` echo for the initiator's own jab
    /// (there was no client click to animate it).
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
        self.dispatch_use_click(s, click, events, true);
    }

    /// Resolve one consumed secondary-button press: build the attempt from
    /// the CLICK's target (never the look latch, which may be newer) and
    /// walk the consumer registry.
    fn dispatch_use_click(
        &mut self,
        s: usize,
        click: PendingUseClick,
        events: &mut TickEvents,
        repeat: bool,
    ) {
        let PendingUseClick {
            mob,
            target,
            request_id,
            predicted,
            jabbed,
            ..
        } = click;
        // The claimed mob resolves through the authoritative view-ray
        // validator BEFORE any consumer (mods included) can observe it: a
        // forged, vanished, dead, or occluded claim is no mob at all.
        let mob = self
            .authoritative_mob_target(s, mob)
            .map(|idx| self.world.mobs().instances()[idx].id());
        let attempt = InteractAttempt {
            block: target.map(|t| t.block),
            face: target.map(|t| t.normal),
            mob,
            player: self.sessions[s].id,
        };
        let meta = ClickMeta {
            target,
            predicted,
            repeat,
        };
        let mut consumed = false;
        let mut placed_at = None;
        for consumer in CONSUMERS {
            match consumer(self, s, &attempt, &meta, events) {
                Claim::Pass => continue,
                Claim::Claimed => {
                    consumed = true;
                }
                Claim::Placed(pos) => {
                    consumed = true;
                    placed_at = Some(pos);
                }
            }
            break;
        }
        // A consumed click whose initiator stayed silent (its replica could
        // not foresee the effect — a mod-claimed attempt like tilling or a
        // right-click harvest) gets its hand jab echoed back; `jabbed`
        // guarantees this can never double an already-played one.
        if consumed && !jabbed {
            events.player(s).used_unpredicted = true;
        }
        // The client's ghost convention is `target.block + normal` — accept
        // ONLY a placement that landed exactly there (accept never rolls
        // back, so anything else must DENY to clear the ghost: a click
        // claimed by an interact/eat/use, a replace-in-place, a slab stack
        // into the hit cell, nothing at all).
        let predicted_cell = target
            .filter(|t| t.normal != IVec3::ZERO)
            .map(|t| t.block + t.normal);
        let accepted = placed_at.is_some() && placed_at == predicted_cell;
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
    /// consumer and emit no action presentation.
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

    /// The mod consumer: dispatch the attempt to every registered
    /// `interact_attempt` handler; a handler's Cancel is a claim. Dispatched
    /// within the sessions view so handlers (and the host calls they make —
    /// `PlayerState`, `Players`) resolve the acting session.
    fn consume_mod_attempt(
        &mut self,
        s: usize,
        attempt: &InteractAttempt,
        _meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if attempt.block.is_none() && attempt.mob.is_none() {
            return Claim::Pass;
        }
        let mut ev = *attempt;
        let claimed = {
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            Self::with_sessions_view(sessions, s, |sess| {
                bus.interact_attempt(
                    world,
                    &mut sess.player,
                    &mut sess.gui_state,
                    events,
                    &mut ev,
                ) == Outcome::Cancel
            })
        };
        if claimed {
            events.player(s).interacted = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    /// Engine shears on the targeted mob.
    fn consume_shear(
        &mut self,
        s: usize,
        attempt: &InteractAttempt,
        _meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if attempt.mob.is_none() {
            return Claim::Pass;
        }
        if self.try_shear_mob(s, attempt.mob) {
            events.player(s).used_item = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    /// The block's built-in capability as a consumer: claims the attempt when
    /// the target block has one AND the shared claim rule says this attempt
    /// is its business (`block::builtin_claims_click` — built-ins pass on
    /// sneak clicks; the client jab/ghost prediction runs the SAME rule).
    fn consume_builtin_block(
        &mut self,
        s: usize,
        attempt: &InteractAttempt,
        _meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        let Some(pos) = attempt.block else {
            return Claim::Pass;
        };
        let block = Block::from_id(self.world.chunk_block(pos.x, pos.y, pos.z));
        if !crate::block::builtin_claims_click(block, self.sessions[s].sneaking()) {
            return Claim::Pass;
        }
        // Menu opens join the ordered menu-action stream. Placement resolves
        // before the Menu stage, so this appends behind any close/click/craft
        // messages already received for the old screen.
        let claimed = match block.interaction() {
            // Engine containers and mod GUIs ride ONE open lane. The clicked
            // block's position rides the session so per-kind session setup
            // (chest viewer, machine gauges, mod container anchoring) and
            // gui_click dispatches know where the GUI was opened from.
            BlockInteraction::OpenGui(kind) => {
                self.sessions[s].pending_menu_actions.push(
                    crate::server::player::PendingMenuAction::OpenGui {
                        kind,
                        pos: Some(pos),
                    },
                );
                true
            }
            // Right-clicking a door toggles it: the open/closed bit flips on this tick
            // (so collision updates at once and the player can step through), and the
            // visual swing is eased from the door's current angle. Seed the swing entry
            // BEFORE the toggle so it starts from the old pose, then eases to the new one.
            BlockInteraction::ToggleDoor => {
                // Act-based claim: a door row whose paired cell cannot be
                // resolved toggles nothing and consumes nothing.
                let Some(lower) = self.world.door_lower_cell(pos.x, pos.y, pos.z) else {
                    return Claim::Pass;
                };
                {
                    self.world.toggle_door(pos);
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

            // Right-clicking a bed sets the spawn point beside it and/or
            // starts the sleep (see `game::bed`); the app opens the sleep
            // overlay via the open request this queues. Act-based claim: a
            // click that moved no spawn and started no sleep consumed
            // nothing.
            BlockInteraction::Sleep => {
                let acted = self.start_sleep(s, pos);
                if acted {
                    events.player(s).bed_interacted = true;
                }
                acted
            }
            BlockInteraction::None => false,
        };
        if claimed {
            events.player(s).interacted = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    /// Whether the held item is BOTH food and placeable (a plantable carrot)
    /// — the dual nature the contextual-place / ordinary-place pair splits on.
    fn held_is_contextual_placeable(&self, s: usize) -> bool {
        self.sessions[s].selected_item().is_some_and(|item| {
            item.food().is_some() && item.as_block().is_some_and(|b| b != Block::Air)
        })
    }

    fn consume_contextual_place(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if !self.held_is_contextual_placeable(s) {
            return Claim::Pass;
        }
        match self.place_held(s, meta.target, meta.predicted, events) {
            Some(pos) => Claim::Placed(pos),
            None => Claim::Pass,
        }
    }

    fn consume_eat(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if !meta.repeat && self.try_start_eating(s, events) {
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    fn consume_item_use(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if self.try_use_item(s, meta.target, events) {
            events.player(s).used_item = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    fn consume_place(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        // A dual-natured item's placement already ran (and passed) above —
        // its click belongs to the eat/use rungs, never a second attempt.
        if self.held_is_contextual_placeable(s) {
            return Claim::Pass;
        }
        match self.place_held(s, meta.target, meta.predicted, events) {
            Some(pos) => Claim::Placed(pos),
            None => Claim::Pass,
        }
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
    ///
    /// [`queue_place_click_for_test`]: Self::queue_place_click_for_test
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
