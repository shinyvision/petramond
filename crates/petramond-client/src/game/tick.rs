use super::Game;
use petramond::net::protocol::{ClientToServer, OpenScreen, PlayerAction, PlayerUpdate, TargetRef};
use petramond_math::math::IVec3;

mod events;
mod replica_clock;

pub use events::{ClientEvents, GameEvents, WorldEvent};
pub use replica_clock::ReplicaClock;

pub use petramond::events::tick::TICK_DT;
pub use petramond::events::tick::{MobSoundEvent, SoundEvent, SpatialSoundCommand};

#[derive(Copy, Clone, Debug)]
pub enum PlacePrediction {
    Predicted(petramond::net::protocol::ClientRequestId),
    TrackOnly(petramond::net::protocol::ClientRequestId),
    Plausible,
    No,
}

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
    pub gameplay_enabled: bool,
    pub movement: MovementInput,
    pub look_delta: (f32, f32),
    pub hotbar_scroll: i32,
    pub break_held: bool,
    pub attack_clicked: bool,
    pub place_clicked: bool,
    pub use_held: bool,
}

impl Game {
    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.tick_send(dt, input);
        self.tick_receive(dt)
    }

    pub fn tick_send(&mut self, dt: f32, input: &GameInput) {
        self.capture_frame_begin();
        self.drive_presentation(dt);
        // Advance render time and turn the interpolation window before the mount slaving below.
        // A batch whose segment the phase just crossed must commit first, or the rider's camera
        // clamps at the segment end for a frame every tick (a 20 Hz stutter). The receive half
        // turns the window again for batches that arrive overdue.
        self.replica.entities.advance_clock(self.world_dt(dt));
        self.advance_interp_window();
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.apply_entity_push(dt);
        self.tick_replica_view();
        self.refresh_target();
        self.update_third_person(dt);
        self.fx.ease_local_bones(
            &self
                .client_mods
                .local_bone_poses(&self.replica.self_view.bone_poses),
            dt,
        );
        let mut tool_input = *input;
        self.world_tool_input(&mut tool_input);
        let input = &tool_input;
        self.tick_local_mining(dt, input);
        self.record_dig(input.break_held);
        self.hand.recover(dt);

        let update = self.build_player_update(input);
        self.build_outgoing_messages(input, update);
        self.local.last_sent_transform = Some(petramond::net::protocol::SelfTransform {
            transform: update.transform,
            on_ground: update.on_ground,
        });
        self.net.flush_frame();
    }

    pub fn tick_receive(&mut self, dt: f32) -> GameEvents {
        let dt = self.world_dt(dt);
        self.pump_network();
        self.bake_client_custom_shapes();
        self.advance_interp_window();

        let replica = &self.replica.world;
        self.replica.entities.advance_animation(dt, |pos| {
            super::body_pose::movement_medium(replica.data(), pos)
        });
        let mut events = std::mem::take(&mut self.replica.events);
        self.deliver_client_mod_events(&events.self_events.client_events);
        self.sync_sleep_camera_on_open(&events.self_events);
        let world = &self.replica.world;
        self.fx.apply_world_effects(world, &events.world);
        let mining = self.replica.self_view.mining.map(|(cell, _)| cell);
        self.fx.tick_mining_dust(world, mining, self.local.look, dt);
        self.fx
            .tick_mob_digging(world, self.replica.entities.mobs(), dt, &mut events.sounds);
        self.fx.tick_particles(world, dt);
        self.fx.advance_block_animations(world, dt);
        self.tick_mesh_budget();

        let mut out = self.assemble_game_events(events, dt);
        out.presented_world_replaced = self.take_world_replaced();
        out.presented_time_jumped = self.take_time_jumped();
        out
    }

    /// Drain and apply every pending server→client message. `Game::tick` runs
    /// it every frame; the App ALSO calls it while a shell screen (pause menu)
    /// suppresses `Game::tick`, so the client keeps consuming server output —
    /// streaming installs land, the channel never backs up, and resume is
    /// instant. Also where a dead server (crash / closed channel) is detected.
    ///
    /// The one place world messages enter — the server's, or what a
    /// presentation released — so it is also where the capture taps them:
    /// after the drain (in order) and around the apply.
    pub fn pump_network(&mut self) {
        self.capture_frame_begin();
        let mut msgs = self.net.drain();
        self.capture_drained(&msgs);
        self.apply_server_messages(&mut msgs);
        self.capture_applied();
        self.net.recycle(msgs);
    }

    fn tick_local_mining(&mut self, dt: f32, input: &GameInput) {
        let tool = self
            .replica
            .self_view
            .inventory
            .selected()
            .and_then(|st| st.tool());
        let look = self.local.look.map(|h| h.block);
        let barred = self
            .local
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Mine);
        self.local.break_repeat.tick(dt);
        if self.local.player.is_creative() {
            self.local.mining = Default::default();
            self.replica.self_view.mining = None;
            if !barred && input.break_held && self.local.break_repeat.ready() {
                if let Some(pos) = look {
                    let block = petramond_world::block::Block::from_id(
                        self.replica.world.data().chunk_block(pos.x, pos.y, pos.z),
                    );
                    if block != petramond_world::block::Block::Air {
                        self.local.break_repeat.arm();
                        let normal = self.local.look.map(|h| h.normal);
                        self.apply_predicted_break(pos, block, normal);
                    }
                }
            }
            return;
        }
        let event = self.local.mining.update(
            dt,
            look,
            input.break_held,
            barred,
            self.replica.world.data(),
            tool,
        );
        self.replica.self_view.mining = self.local.mining.overlay();

        if let Some(ev) = event {
            let normal = self
                .local
                .look
                .filter(|h| h.block == ev.pos && h.normal != IVec3::ZERO)
                .map(|h| h.normal);
            self.apply_predicted_break(ev.pos, ev.block, normal);
        }
    }

    fn assemble_game_events(&mut self, events: ClientEvents, dt: f32) -> GameEvents {
        let se = events.self_events;
        let hand = self
            .hand
            .take_frame(se.used_unpredicted, se.used_unpredicted_off);
        let animator_events = self
            .client_mods
            .take_animator_events(&se.animator_events, dt);
        let mut out = GameEvents {
            animator_events,
            placed_block: hand.placed,
            placed_off_hand: hand.placed_off_hand,
            broke_block: hand.broke,
            swung_hand: hand.swung,
            picked_up_item: se.picked_up_item,
            threw_item: hand.threw,
            close_document_gui: se.close_document_gui,
            toggled_panel: se.toggled_panel,
            bed_interacted: se.bed_interacted,
            interacted: hand.interacted,
            interacted_off_hand: hand.interacted_off_hand,
            interacted_presents_itself: hand.interacted_presents_itself,
            interacted_places: hand.interacted_places,
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
        out.connection_lost = self.net.take_lost_report();
        match se.open_screen {
            None => {}
            Some(OpenScreen::Gui { kind_key, anchor }) => {
                // An unknown kind is a mod the client lacks. The handshake makes this unreachable
                // in practice, so skip rather than panic. An inventory open is the server's ack of
                // our own E-key request, whose screen is already up, so it never re-opens.
                match petramond_world::gui_state::resolve_kind(&kind_key) {
                    Some(kind) if kind != petramond_world::gui_state::GuiKind::Inventory => {
                        out.open_gui = Some((kind, anchor));
                    }
                    _ => {}
                }
            }
            Some(OpenScreen::Sleep) => out.open_sleep = true,
        }
        self.hand.latch_swing_events(out.one_shots());
        out
    }

    fn build_player_update(&self, input: &GameInput) -> PlayerUpdate {
        let intent = self.local.predicted_input;
        PlayerUpdate {
            transform: petramond::net::protocol::Transform {
                pos: self.local.player.pos,
                vel: self.local.player.vel,
                yaw: self.local.player.yaw,
                pitch: self.local.player.pitch,
            },
            on_ground: self.local.player.on_ground,
            sneak: intent.sneak,
            gameplay: input.gameplay_enabled,
            break_held: input.break_held,
            use_held: input.use_held,
            target: self.local.look.map(|h| TargetRef::of_hit(&h)),
            hotbar_slot: self.local.player.inventory.active_slot(),
            held_rotation: self.local.held_rotation.rotation,
            wishdir: intent.wishdir,
            jump: intent.jump,
            sprint: intent.sprint,
        }
    }

    /// The whole use-click prediction verdict: the SHARED consumer walk
    /// (`rules::use_click` — the server's own two-pass ladder and registry
    /// order) with every rung answered against the replica (see
    /// [`predict_use_click`](Self::predict_use_click)). The MAIN hand first;
    /// only when nothing is predicted to act does the OFF hand evaluate. At
    /// most one pass opens a ledger entry: a place ghost claims its rung and
    /// ends the walk.
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
        let outcome = self.predict_use_click(input.movement.sneak, use_mob);
        let place = outcome.placement.unwrap_or(PlacePrediction::No);
        let consumed = outcome.consumed();
        if !consumed {
            let payload = mod_api::EventPayload::UseUnclaimed {
                block: self.local.look.map(|h| h.block.to_array()),
                face: self.local.look.map(|h| h.normal.to_array()),
                mob: use_mob,
                player: mod_api::PlayerId(self.replica.entities.self_id().0),
            };
            self.predict_mod_claim(input.movement.sneak, payload);
        }
        self.local.player.use_gesture = match self.client_mods.use_holder() {
            Some(owner) => petramond::player::UseGesture::Held(owner.into()),
            None => petramond::player::UseGesture::Free,
        };
        ClickVerdict {
            consumed,
            presents_itself: outcome.presents_itself(),
            places: !matches!(place, PlacePrediction::No),
            off_hand: outcome.off_hand_acted(),
            place,
        }
    }

    fn attack_press(&mut self, input: &GameInput) -> bool {
        let denied = self
            .local
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Attack);
        let mining = self.replica.self_view.mining.is_some();
        let player = &self.local.player;
        self.hand
            .attack_press(denied, mining, input.attack_clicked, || {
                petramond::events::tick::TICK_DT
                    * player.scaled_ticks(
                        mod_api::PlayerAttribute::AttackCooldown,
                        petramond::rules::combat::ATTACK_COOLDOWN_TICKS,
                    ) as f32
            })
    }

    fn build_outgoing_messages(&mut self, input: &GameInput, update: PlayerUpdate) {
        debug_assert!(self.net.frame_is_empty(), "pump drains every frame");
        let use_mob = input
            .place_clicked
            .then(|| self.targeted_mob_id())
            .flatten();
        let attacks = input.gameplay_enabled && self.attack_press(input);
        let attack_mob = attacks.then(|| self.targeted_mob_id()).flatten();
        let attack_player = attacks.then_some(self.local.targeted_player).flatten();
        self.net.push_frame(ClientToServer::PlayerUpdate(update));
        if input.gameplay_enabled {
            let denied = self.local.player.denied_actions();
            if input.place_clicked && !denied.denies(mod_api::BodyAction::Use) {
                let target = self.local.use_look.map(|h| TargetRef::of_hit(&h));
                let verdict = self.predict_click_verdict(input, use_mob);
                let request_id = match verdict.place {
                    PlacePrediction::Predicted(id) | PlacePrediction::TrackOnly(id) => Some(id),
                    _ => None,
                };
                self.hand.latch_use(super::local_hand::UseJab {
                    consumed: verdict.consumed,
                    off_hand: verdict.off_hand,
                    presents_itself: verdict.presents_itself,
                    places: verdict.places,
                });
                self.net
                    .push_frame(ClientToServer::Action(PlayerAction::UseClick {
                        mob: use_mob,
                        target,
                        request_id,
                        predicted: matches!(verdict.place, PlacePrediction::Predicted(_)),
                        jabbed: verdict.consumed,
                    }));
            }
            if attacks {
                self.hand.latch_swing();
                self.net
                    .push_frame(ClientToServer::Action(PlayerAction::AttackClick {
                        mob: attack_mob,
                        player: attack_player,
                    }));
            }
        }
        self.poll_schematic_share();
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
    /// can only ever carry the one-RTT-old echo of our own look. Adopting
    /// that echo during a stream of corrections would revert look changes
    /// before the server sees them, breaking actions that depend on the
    /// server's view ray.
    /// If a server feature must one day force a look, it needs an explicit
    /// signal on the wire, not this echo channel.
    pub fn adopt_authoritative_transform(&mut self, t: &petramond::net::protocol::SelfTransform) {
        let sent = self.local.last_sent_transform;
        if sent.is_none_or(|s| s.transform.pos != t.transform.pos) {
            self.local.player.teleport(t.transform.pos);
        }
        if sent.is_none_or(|s| s.transform.vel != t.transform.vel) {
            self.local.player.vel = t.transform.vel;
        }
        if sent.is_none_or(|s| s.on_ground != t.on_ground) {
            self.local.player.on_ground = t.on_ground;
        }
    }
}
