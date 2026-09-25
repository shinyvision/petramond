//! The frame→tick boundary types shared by the client and the server sim
//! ([`GameInput`], [`GameEvents`], `TickEvents`/`WorldEvents`) plus the
//! client's per-frame [`Game::tick`] driver. The fixed-tick stage ladder
//! itself lives on [`petramond::server::game::ServerGame`].

use super::world_prediction::UseClaim;
use super::Game;
use petramond::net::protocol::{ClientToServer, OpenScreen, PlayerAction, PlayerUpdate, TargetRef};
use petramond::player::one_shot::OneShot;
use petramond::server::interact::ConsumerKind;
use petramond_math::math::IVec3;
use petramond_world::inventory::Hand;

mod events;
mod replica_clock;

pub use events::{ClientEvents, GameEvents, WorldEvent};
pub use replica_clock::ReplicaClock;

pub use petramond::events::tick::TICK_DT;
pub use petramond::events::tick::{MobSoundEvent, SoundEvent, SpatialSoundCommand};

/// What the place-prediction pass decided for a use click (see
/// `Game::try_predict_place_ghost`). Distinguishing `Plausible` from `No`
/// matters twice: the P0 hand jab fires for any click that will likely place,
/// and the wire's `UseClick.predicted` flag (true only for `Predicted`) tells
/// the server whether to strip the initiator's `BlockPlaced` echo.
#[derive(Copy, Clone, Debug)]
pub enum PlacePrediction {
    /// Full P1: replica cells written, presentation played, ledger entry open.
    Predicted(petramond::net::protocol::ClientRequestId),
    /// Ledger frozen: the id ships (and is answered) but nothing presented.
    TrackOnly(petramond::net::protocol::ClientRequestId),
    /// The server will likely place, but off the ghost convention
    /// (`target + normal`) — replace-in-place, a slab stack into the hit
    /// cell, an oriented model's shifted base — so no ghost is drawn.
    Plausible,
    /// The click won't place anything.
    No,
}

/// A use click's predicted verdict: whether something consumes it, whether
/// that consumer presents itself (an eat's raise, not a jab) or is a
/// placement (the place jab, not the interact one), which hand acted, and
/// what the place prediction did.
pub struct ClickVerdict {
    pub consumed: bool,
    pub presents_itself: bool,
    pub places: bool,
    pub off_hand: bool,
    pub place: PlacePrediction,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MovementInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct GameInput {
    /// False while an app screen such as inventory owns input focus.
    pub gameplay_enabled: bool,
    pub movement: MovementInput,
    pub look_delta: (f32, f32),
    /// Whole wheel notches scrolled this frame (signed): negative selects
    /// previous slots, positive selects next, 0 for none. Wraps within the hotbar.
    pub hotbar_scroll: i32,
    /// Level state: primary button held for mining.
    pub break_held: bool,
    /// Edge state: primary button *pressed* this frame.
    pub attack_clicked: bool,
    /// Edge state: secondary button pressed for placement.
    pub place_clicked: bool,
    /// Level state: secondary button held — sustains an in-progress eat.
    pub use_held: bool,
}

impl Game {
    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.tick_send(dt, input);
        self.tick_receive(dt)
    }

    /// The frame's INPUT half: per-frame client systems, then this frame's
    /// protocol messages onto the server channel — the ONLY road input takes
    /// into the sim (the server thread latches them before its next ticks).
    /// Split from [`tick_receive`](Self::tick_receive) so the test harness can
    /// service the pipe synchronously between the halves (production runs
    /// them back-to-back; the thread answers asynchronously).
    pub fn tick_send(&mut self, dt: f32, input: &GameInput) {
        // Render time advances by real frame dt FIRST, and the interpolation
        // window turns HERE too — a batch staged last frame whose segment the
        // phase just crossed must commit BEFORE the mount slaving below
        // samples it, or the rider's camera clamps at the segment end for one
        // frame every tick (a 20 Hz stutter) while the world glides on. The
        // receive half turns it again for batches that arrive already overdue.
        self.replica_clock.advance(dt);
        self.advance_interp_window();
        // Per-frame exceptions kept for local feel: look, hotbar, local player, entity push.
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.apply_entity_push(dt);
        self.tick_replica_view();
        self.refresh_target();
        self.update_third_person(dt);
        // The local body's bones ease at the same rate as the item in its fist
        // — the local player sees their own arms in third person, and an arm
        // that snaps while its shield glides is the shield trailing its own
        // hand.
        let mut target = std::mem::take(&mut self.local_bone_target);
        target.clear();
        super::render_bone_offsets(
            &self
                .client_mods
                .local_bone_poses(&self.self_view.bone_poses),
            &mut target,
        );
        self.local_bones.advance(&target, dt);
        self.local_bone_target = target;
        let mut tool_input = *input;
        self.world_tool_input(&mut tool_input);
        let input = &tool_input;
        self.tick_local_mining(dt, input);
        self.local_attack_recovery = (self.local_attack_recovery - dt).max(0.0);

        let update = self.build_player_update(input);
        self.build_outgoing_messages(input, update);
        // What we are claiming this frame — the reference a later
        // `SelfState::transform` correction diffs against (fields the server
        // merely echoes back must not stomp newer local values).
        self.last_sent_transform = Some(petramond::net::protocol::SelfTransform {
            transform: update.transform,
            on_ground: update.on_ground,
        });
        let mut lost = false;
        for msg in self.frame_messages.drain(..) {
            if lost {
                continue; // drain the rest; the server is gone
            }
            lost = self.handle.send(msg).is_err();
        }
        if lost {
            self.note_connection_lost();
        }
    }

    /// The frame's OUTPUT half: drain + apply the server's messages, then the
    /// per-frame presentation systems, and assemble the app-facing events.
    pub fn tick_receive(&mut self, dt: f32) -> GameEvents {
        // Apply the drained messages (terrain installs, then tick batches)
        // BEFORE any presentation/HUD read — the same messages a remote
        // client applies off the wire. Everything below consumes ONLY what
        // those messages carried (`ClientEvents`), never sim state. More than
        // one `TickUpdate` may have landed since last frame; `ClientEvents`
        // accumulates their immediate payloads (one-shots OR, queues append,
        // latest read-model state wins), while entity rows enter their bounded
        // interpolation FIFO.
        self.pump_network();
        // Bake any custom-shape cells the ingested deltas dirtied, so the
        // next frame's client physics/prediction reads real (server-matching)
        // collision instead of the static fallback.
        self.bake_client_custom_shapes();
        // Turn the staged interpolation window now that the batches drained,
        // so presentation below samples committed pairs + a settled alpha —
        // then ease the named-animation blend weights toward the committed
        // target sets (oars pick up / settle instead of snapping).
        self.advance_interp_window();
        self.replicated_mobs.advance_anim_blends(dt);

        // Presentation/infra after fixed simulation; no gameplay mutation here.
        // Remote players' per-frame animation state (shared body pose,
        // held-item easing, animator plays, hurt/eat ramps) advances right after the batches
        // applied, so this frame's latched one-shots jab this frame.
        let alpha = self.replica_clock.alpha();
        self.remote_players.advance(dt, alpha, |pos| {
            super::body_pose::movement_medium(&self.replica, pos)
        });
        let events = std::mem::take(&mut self.pending_events);
        self.deliver_client_mod_events(&events.self_events.client_events);
        self.sync_sleep_camera_on_open(&events.self_events);
        // World-anchored effects the tick batch carried (break bursts, door
        // swings) spawn from the replicated events — the identical path a
        // remote client drives them from off the wire.
        self.apply_world_effects(&events.world);
        self.tick_mining_dust(dt);
        let digging = self.tick_mob_digging(dt);
        self.tick_entities(dt);
        self.advance_chest_lids(dt);
        self.advance_panel_swings(dt);
        self.tick_mesh_budget();

        let mut out = self.assemble_game_events(events, dt);
        out.sounds.extend(digging);
        out
    }

    /// Drain and apply every pending server→client message. `Game::tick` runs
    /// it every frame; the App ALSO calls it while a shell screen (pause menu)
    /// suppresses `Game::tick`, so the client keeps consuming server output —
    /// streaming installs land, the channel never backs up, and resume is
    /// instant. Also where a dead server (crash / closed channel) is detected.
    pub fn pump_network(&mut self) {
        if self.handle.is_crashed() {
            self.note_connection_lost();
        }
        let mut msgs = std::mem::take(&mut self.incoming);
        self.handle.drain(&mut msgs);
        self.apply_server_messages(&mut msgs);
        self.incoming = msgs; // drained; capacity reused
    }

    /// Advance the local mining timer for crack overlay and emit
    /// `BreakFinished` when the client finishes a break. On finish the client
    /// fully applies the break it can see (cell clear, hand, local world
    /// event).
    fn tick_local_mining(&mut self, dt: f32, input: &GameInput) {
        let tool = self.self_view.inventory.selected().and_then(|st| st.tool());
        let look = self.look.map(|h| h.block);
        // One question, the same one the server asks: a body barred from mining
        // stops predicting one, whether the bar is a pack's claim or the open
        // menu the engine claims for. Without it the crack creeps up a block
        // the authority already refused and then snaps back.
        let barred = self
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Mine);
        self.break_repeat.tick(dt);
        if self.player.is_creative() {
            self.local_mining = Default::default();
            self.self_view.mining = None;
            if !barred && input.break_held && self.break_repeat.ready() {
                if let Some(pos) = look {
                    let block = petramond_world::block::Block::from_id(
                        self.replica.chunk_block(pos.x, pos.y, pos.z),
                    );
                    if block != petramond_world::block::Block::Air {
                        self.break_repeat.arm();
                        let normal = self.look.map(|h| h.normal);
                        self.apply_predicted_break(pos, block, normal);
                    }
                }
            }
            return;
        }
        let event =
            self.local_mining
                .update(dt, look, input.break_held, barred, &self.replica, tool);
        // The own crack overlay is CLIENT-OWNED: the local timer is its only
        // source (the server never ships it back — SelfState carries no
        // `mining` echo).
        self.self_view.mining = self.local_mining.overlay();

        if let Some(ev) = event {
            let normal = self
                .look
                .filter(|h| h.block == ev.pos && h.normal != IVec3::ZERO)
                .map(|h| h.normal);
            self.apply_predicted_break(ev.pos, ev.block, normal);
        }
    }

    /// Map this frame's replicated event payloads onto the app-facing
    /// `GameEvents` shape (the app's consumption is unchanged: the one-shots
    /// and `open_*` fields read exactly as they did pre-wire). `dt` is the
    /// frame's seconds.
    fn assemble_game_events(&mut self, events: ClientEvents, dt: f32) -> GameEvents {
        let se = events.self_events;
        // The hand one-shots are fed by the local prediction latches — the
        // server never echoes an action the client already animated. The one
        // exception is `used_unpredicted`: a consumed click whose shipped
        // `jabbed` verdict was silent (a mod-consumed use/interact the
        // replica can't foresee), so folding it in can never play twice.
        let local_jab = std::mem::take(&mut self.local_hand_jab) || se.used_unpredicted;
        // Which hand the jab belongs to: the local latch's own verdict, or —
        // for the echoed unpredicted consumption — the server's acting hand.
        let local_jab_off = std::mem::take(&mut self.local_hand_jab_off) || se.used_unpredicted_off;
        // An echoed consumption never presents itself: the server echoes no
        // such claim, and the client always foresees held food.
        let local_presents_itself =
            std::mem::take(&mut self.local_hand_presents_itself) && !se.used_unpredicted;
        let local_places = std::mem::take(&mut self.local_hand_places) && !se.used_unpredicted;
        let local_swing = std::mem::take(&mut self.local_hand_swing);
        let local_threw = std::mem::take(&mut self.local_hand_threw);
        let local_broke = std::mem::take(&mut self.local_broke_block);
        let local_placed = std::mem::take(&mut self.local_placed_block);
        let local_placed_off = std::mem::take(&mut self.local_placed_off_hand);
        let animator_events = self
            .client_mods
            .take_animator_events(&se.animator_events, dt);
        let mut out = GameEvents {
            animator_events,
            placed_block: local_placed,
            placed_off_hand: local_placed_off,
            broke_block: local_broke,
            swung_hand: local_swing,
            picked_up_item: se.picked_up_item,
            threw_item: local_threw,
            close_document_gui: se.close_document_gui,
            toggled_panel: se.toggled_panel,
            bed_interacted: se.bed_interacted,
            interacted: local_jab,
            interacted_off_hand: local_jab && local_jab_off,
            interacted_presents_itself: local_jab && local_presents_itself,
            interacted_places: local_jab && local_places,
            player_damaged: se.player_damaged,
            player_died: se.player_died,
            sleep_ended: se.sleep_ended,
            respawned: se.respawned,
            sounds: events.sounds,
            spatial_sounds: events.spatial_sounds,
            mob_sounds: events.mob_sounds,
            world_events: events.world,
            ..Default::default()
        };
        if !self.connection_lost_reported {
            if let Some(reason) = &self.connection_lost {
                log::error!("{reason}; nothing further will be saved");
                out.connection_lost = Some(reason.clone());
                self.connection_lost_reported = true;
            }
        }
        match se.open_screen {
            None => {}
            Some(OpenScreen::Gui { kind_key, anchor }) => {
                // Unknown kind = a mod the client lacks; the handshake makes
                // this unreachable in practice — skip rather than panic. The
                // inventory open is the server's ack of the client's own E-key
                // request (its screen is already up), so it never re-opens.
                match petramond_world::gui_state::resolve_kind(&kind_key) {
                    Some(kind) if kind != petramond_world::gui_state::GuiKind::Inventory => {
                        out.open_gui = Some((kind, anchor));
                    }
                    _ => {}
                }
            }
            Some(OpenScreen::Sleep) => out.open_sleep = true,
        }
        // The hand one-shots, latched AGAIN for the client-mod frame hook
        // (`Game::swing_events`): the app's own latch feeds the animators
        // and drains at render, so a consumer on the update clock needs its
        // own copy — a shared latch is whoever-eats-first, and the frame
        // hook always ate second. The first gesture in `one_shots` order
        // wins a hand; the ranking is cosmetic (the edges share one button,
        // so they almost never coincide).
        for (hand, kind) in out.one_shots() {
            let slot = match hand {
                Hand::Main => &mut self.swing_events.main,
                Hand::Off => &mut self.swing_events.off,
            };
            if slot.is_none() {
                *slot = Some(match kind {
                    OneShot::Swing => mod_api::SwingKind::Attack,
                    OneShot::Break => mod_api::SwingKind::Break,
                    OneShot::Place => mod_api::SwingKind::Place,
                    OneShot::Throw => mod_api::SwingKind::Throw,
                    OneShot::Interact => mod_api::SwingKind::Interact,
                });
            }
        }
        out
    }

    /// This frame's transform + held-intent message, built from the predicted
    /// player and the per-frame targeting. Held intents ride raw — the server
    /// forces them off while `gameplay` is false (the old `capture_intent`
    /// rule); the held-rotation counter rides raw and the session re-derives
    /// the armed item (see `HeldRotation::apply_wire`).
    fn build_player_update(&self, input: &GameInput) -> PlayerUpdate {
        // The movement intent is EXACTLY what this frame's local physics
        // consumed (`tick_player` stashes it) — re-deriving it here would read
        // the camera after `sync_camera_to_player_eye` moved it and drift the
        // wire intent from the prediction.
        let intent = self.predicted_input;
        PlayerUpdate {
            transform: petramond::net::protocol::Transform {
                pos: self.player.pos,
                vel: self.player.vel,
                yaw: self.player.yaw,
                pitch: self.player.pitch,
            },
            on_ground: self.player.on_ground,
            // Sneak is part of the F2 movement intent now: ship EXACTLY what the
            // local physics consumed (gameplay-gated), so the server integrates
            // the same edge-guarded, half-speed step the prediction ran.
            sneak: intent.sneak,
            gameplay: input.gameplay_enabled,
            break_held: input.break_held,
            use_held: input.use_held,
            target: self.look.map(|h| TargetRef::of_hit(&h)),
            hotbar_slot: self.player.inventory.active_slot(),
            held_rotation: self.held_rotation.rotation,
            wishdir: intent.wishdir,
            jump: intent.jump,
            sprint: intent.sprint,
        }
    }

    /// The whole use-click prediction verdict, mirroring the server's
    /// two-pass ladder: the MAIN hand first; only when nothing is predicted
    /// to act does the OFF hand evaluate (acting hand = actor context,
    /// exactly like the server's dispatch — every held read in the
    /// prediction resolves through `player.acting_hand`). At most one pass
    /// opens a ledger entry: pass 2 runs only when pass 1 predicted nothing
    /// at all. Returns `(jabbed, off_hand_acted, place)`.
    ///
    /// The jab fires when the click predictably does something — including a
    /// PLAUSIBLE placement the ghost convention keeps unpredicted (oriented
    /// model, replace-in-place, slab stack). A click the client knows is a
    /// no-op still ships (the server may know better — mods, state the
    /// replica can't see) and stays silent locally; the verdict rides the
    /// wire so a server-side surprise (a mod-consumed use/interact) echoes
    /// the jab back through `SelfEvents::used_unpredicted`.
    pub(super) fn predict_click_verdict(
        &mut self,
        input: &GameInput,
        use_mob: Option<u64>,
    ) -> ClickVerdict {
        self.player.acting_hand = Hand::Main;
        let (mod_claimed, mut place) = self.predict_use_click(input.movement.sneak, use_mob);
        let mut claim = if mod_claimed {
            UseClaim::Claimed(ConsumerKind::Registered)
        } else if !matches!(place, PlacePrediction::No) {
            UseClaim::Claimed(ConsumerKind::Place)
        } else {
            self.use_click_claim(input, use_mob)
        };
        let mut off_hand = false;
        if claim == UseClaim::Unclaimed && self.self_view.inventory.off_hand().is_some() {
            self.player.acting_hand = Hand::Off;
            let (mod_claimed_off, place_off) =
                self.predict_use_click(input.movement.sneak, use_mob);
            let off_claim = if mod_claimed_off {
                UseClaim::Claimed(ConsumerKind::Registered)
            } else if !matches!(place_off, PlacePrediction::No) {
                UseClaim::Claimed(ConsumerKind::Place)
            } else {
                self.use_click_claim(input, use_mob)
            };
            if off_claim != UseClaim::Unclaimed {
                place = place_off;
                claim = off_claim;
                off_hand = true;
            }
        }
        // Never leak the acting hand past the verdict (level-state reads —
        // the render frame, the roster — are main-hand by definition).
        self.player.acting_hand = Hand::Main;
        let consumed = claim != UseClaim::Unclaimed;
        // NOTHING claimed it: offer the gesture, exactly as the server does
        // once its own chain has passed. This is the frame a guard goes up on.
        if !consumed {
            let payload = mod_api::EventPayload::UseUnclaimed {
                block: self.look.map(|h| h.block.to_array()),
                face: self.look.map(|h| h.normal.to_array()),
                mob: use_mob,
                player: mod_api::PlayerId(self.self_id.0),
            };
            // The verdict is NOT a jab: nothing happened to the world, and
            // whoever took the gesture poses the body itself. Predicting a
            // swing here would punch the air every time a guard went up.
            // Through the scoped dispatch: a rule that gates on what the
            // pack holds (a launcher with nothing to launch) reads the
            // replicated inventory.
            self.predict_mod_claim(input.movement.sneak, payload);
        }
        // Mirror whoever took it onto the body, so the predicted player answers
        // "is this press spoken for" the way the authority will.
        self.player.use_gesture = match self.client_mods.use_holder() {
            Some(owner) => petramond::player::UseGesture::Held(owner.into()),
            None => petramond::player::UseGesture::Free,
        };
        ClickVerdict {
            consumed,
            presents_itself: claim.presents_itself(),
            places: !matches!(place, PlacePrediction::No),
            off_hand,
            place,
        }
    }

    /// Whether the primary button swings the hand THIS frame: a fresh press,
    /// or one held over from the swing still following through.
    ///
    /// A swing plays WHOLE — `local_attack_recovery` mirrors the server's
    /// attack window, scaled by the same claimed attribute (a pack pacing a
    /// tool off its own animation claims it to zero and paces the hand
    /// itself, so its mid-arc press flows straight through to its clock).
    /// A press landing inside that window is neither spent nor predicted on
    /// the spot — it is HELD, one deep, and fires by itself the frame the
    /// hand comes home, exactly as a perfectly timed click would. The server
    /// holds the press it receives the same way, so a queued swing cannot
    /// die in the gap between the two clocks.
    ///
    /// A mining press swings nothing (that arc is the dig loop's), so it
    /// neither arms the recovery nor is held by one — packs listen for that
    /// echo on every press.
    fn attack_press(&mut self, input: &GameInput) -> bool {
        if self
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Attack)
        {
            // A denied action did not happen: nothing of it is held over.
            self.local_attack_queued = false;
            return false;
        }
        if self.self_view.mining.is_some() {
            return input.attack_clicked;
        }
        if self.local_attack_recovery > 0.0 {
            self.local_attack_queued |= input.attack_clicked;
            return false;
        }
        if !input.attack_clicked && !self.local_attack_queued {
            return false;
        }
        self.local_attack_queued = false;
        self.local_attack_recovery = petramond::events::tick::TICK_DT
            * self.player.scaled_ticks(
                mod_api::PlayerAttribute::AttackCooldown,
                petramond::server::game::ATTACK_COOLDOWN_TICKS,
            ) as f32;
        true
    }

    /// Assemble this frame's message batch into `frame_messages`, in
    /// consumption order: the `PlayerUpdate` first (so the edge-drop rule and
    /// slot-dependent actions see this frame's state), then this frame's click
    /// edges (mob targets resolved to STABLE ids now, at click time), then
    /// everything the app-facing methods queued since the last frame.
    fn build_outgoing_messages(&mut self, input: &GameInput, update: PlayerUpdate) {
        debug_assert!(self.frame_messages.is_empty(), "pump drains every frame");
        let use_mob = input
            .place_clicked
            .then(|| self.targeted_mob_id())
            .flatten();
        // The swing this frame: a fresh press, or the one held over from the
        // hand's follow-through. Resolved BEFORE the targets so a queued
        // press takes the crosshair it fires at, not the one it was pressed
        // at — the authority validates the target at the same instant.
        let attacks = input.gameplay_enabled && self.attack_press(input);
        let attack_mob = attacks.then(|| self.targeted_mob_id()).flatten();
        // At most one of mob/player is targeted per frame (refresh_target's
        // nearest-wins pick), so the click carries at most one.
        let attack_player = attacks.then_some(self.targeted_player).flatten();
        self.frame_messages
            .push(ClientToServer::PlayerUpdate(update));
        if input.gameplay_enabled {
            // A barred body sends no click, runs no prediction and plays no
            // jab: the server would spend the press for nothing, so predicting
            // one would put a place ghost on screen that the next batch takes
            // straight back. The button is dead on both mirrors at once.
            let denied = self.player.denied_actions();
            if input.place_clicked && !denied.denies(mod_api::BodyAction::Use) {
                // The click's block target rides the wire: the server resolves
                // the interact/place against THIS cell, never a fresher look —
                // a click racing the crosshair must land where the ghost is.
                // `use_look` == `look` unless the held item declares a
                // water-stopping use ray (see `refresh_target`).
                let target = self.use_look.map(|h| TargetRef::of_hit(&h));
                let verdict = self.predict_click_verdict(input, use_mob);
                let request_id = match verdict.place {
                    PlacePrediction::Predicted(id) | PlacePrediction::TrackOnly(id) => Some(id),
                    _ => None,
                };
                self.local_hand_jab = verdict.consumed;
                self.local_hand_jab_off = verdict.consumed && verdict.off_hand;
                self.local_hand_presents_itself = verdict.presents_itself;
                self.local_hand_places = verdict.places;
                self.frame_messages
                    .push(ClientToServer::Action(PlayerAction::UseClick {
                        mob: use_mob,
                        target,
                        request_id,
                        predicted: matches!(verdict.place, PlacePrediction::Predicted(_)),
                        jabbed: verdict.consumed,
                    }));
            }
            // Same for the swing — it would be the one visible thing a denied
            // action is allowed to leave behind ([`Self::attack_press`] reads
            // the same denial).
            if attacks {
                self.local_hand_swing = true;
                self.frame_messages
                    .push(ClientToServer::Action(PlayerAction::AttackClick {
                        mob: attack_mob,
                        player: attack_player,
                    }));
            }
        }
        self.poll_schematic_share();
        self.frame_messages.append(&mut self.outbox);
    }

    /// Adopt a `SelfState::transform` correction: the server's ticks moved
    /// this player (bed tuck, wake/respawn teleports, mod `Teleport`,
    /// mob-strike knockback). Per-field against the transform we last SENT:
    /// a field still equal to our last claim is the server echoing us — the
    /// local value (possibly a frame newer: movement) wins; a differing
    /// field is a genuine server-side mutation. A position change adopts via
    /// `Player::teleport` so the client's own fall bookkeeping re-anchors too.
    /// Without a `last_sent_transform` (before the first frame) everything
    /// adopts — the values are the shared restore, so it is a no-op.
    ///
    /// YAW/PITCH ARE NEVER ADOPTED: the look is client-owned INPUT (like the
    /// hotbar index), not physics — no server tick steers it, so a correction
    /// can only ever carry the one-RTT-old echo of our own look. The old
    /// per-field adoption made that echo look like a server-side set the
    /// moment the player turned: while a correction STREAM flows (claims
    /// rejected over not-yet-final terrain — routine when two players drag
    /// far-apart streaming windows), every look change was reverted before
    /// the server could see it, which silently killed everything that needs
    /// the server's own view ray to agree (the `use_ray: water` boat click).
    /// If a server feature must one day force a look, it needs an explicit
    /// signal on the wire, not this echo channel.
    pub fn adopt_authoritative_transform(&mut self, t: &petramond::net::protocol::SelfTransform) {
        let sent = self.last_sent_transform;
        if sent.is_none_or(|s| s.transform.pos != t.transform.pos) {
            self.player.teleport(t.transform.pos);
        }
        if sent.is_none_or(|s| s.transform.vel != t.transform.vel) {
            self.player.vel = t.transform.vel;
        }
        if sent.is_none_or(|s| s.on_ground != t.on_ground) {
            self.player.on_ground = t.on_ground;
        }
    }
}
