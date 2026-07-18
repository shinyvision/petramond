use crate::events::{Attach, PostEvent, SessionPlayerRef, SimCtx, Stage};
use crate::game::tick::{TickEvents, TICK_DT};
use crate::server::player::{ConnectedPlayer, PlayerId};

use super::{ServerGame, MAX_TICKS_PER_FRAME};

impl ServerGame {
    /// Run the fixed ticks `dt` banked. Returns the events plus how many ticks
    /// actually executed (the pump emits a replication batch only when > 0).
    pub(crate) fn run_fixed_ticks(&mut self, dt: f32) -> (TickEvents, u32) {
        // Clamp long stalls and cap catch-up so fixed ticks never spiral.
        self.tick_accumulator += dt.clamp(0.0, 1.0);
        let mut ran = 0;
        let mut events = TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle);
        while self.tick_accumulator >= TICK_DT && ran < MAX_TICKS_PER_FRAME {
            self.game_tick_step(&mut events);
            self.tick_accumulator -= TICK_DT;
            ran += 1;
        }
        if self.tick_accumulator > TICK_DT {
            self.tick_accumulator = TICK_DT;
        }
        self.next_mod_sound_handle = events.next_spatial_sound_handle();
        (events, ran)
    }

    /// One fixed game tick: world and entity mutation only. The hardwired engine
    /// steps run in [`Stage`] order; between them the scheduler runs attached
    /// systems and the post-event queue drains (see [`end_stage`](Self::end_stage)).
    /// `pub(crate)` so tests can drive exactly one tick.
    pub(crate) fn game_tick_step(&mut self, events: &mut TickEvents) {
        // Victim-owned i-frames advance before ANY source can deal damage,
        // including mod actions queued at the previous drain point.
        self.tick_damage_immunity();

        // Post events queued from per-frame code since the last tick (section
        // stream installs, container screens) dispatch first, before any stage:
        // per-frame code only ever queues; handlers run on the tick. Mod
        // actions still queued from the previous tick's final drain (or from
        // mod_init) apply here first.
        self.pump_stream_events();
        self.publish_dismounted();
        self.apply_mod_actions(events);
        self.drain_post_events(events);

        // Keep action intent before world/entity simulation so inputs resolve
        // on the tick. Per-player stages loop the sessions in id order INSIDE
        // the stage, so the mod seams (`begin_stage`/`end_stage`) still run
        // once per stage per tick. The published input snapshots (the
        // `PlayerInput` HostCall's read model) capture the same intents the
        // movement integration consumes.
        self.publish_player_inputs();
        self.tick_movements();

        self.begin_stage(Stage::Mining, events);
        for s in 0..self.sessions.len() {
            self.tick_mining(s, events);
        }
        self.end_stage(Stage::Mining, events);

        self.begin_stage(Stage::Placement, events);
        for s in 0..self.sessions.len() {
            self.tick_place(s, events);
        }
        self.end_stage(Stage::Placement, events);

        self.begin_stage(Stage::Attack, events);
        for s in 0..self.sessions.len() {
            self.tick_attack(s, events);
        }
        self.end_stage(Stage::Attack, events);

        self.begin_stage(Stage::Drops, events);
        for s in 0..self.sessions.len() {
            self.tick_drops(s, events);
        }
        self.end_stage(Stage::Drops, events);

        self.begin_stage(Stage::Menu, events);
        for s in 0..self.sessions.len() {
            self.tick_menu(s, events);
        }
        self.end_stage(Stage::Menu, events);

        self.begin_stage(Stage::PlayerDamage, events);
        for s in 0..self.sessions.len() {
            self.tick_fall_damage(s, events);
            self.tick_water_splash(s, events);
            // Status effects ride the same stage: they are pure player-state
            // steps (regen heals, durations count down) on the tick, after damage
            // so a same-tick hit lands before the heal.
            self.tick_effects(s);
            // Sleeping and respawn ride the same stage: both are pure player-state
            // transitions (teleport, health restore, time skip) on the tick.
            self.tick_bed_and_respawn(s, events);
        }
        // Sleep completion is a cross-player decision (everyone must sleep),
        // resolved once after every session advanced its own timer.
        self.resolve_sleep_completion(events);
        self.end_stage(Stage::PlayerDamage, events);

        // World::game_tick's internal order (scheduled → block updates → furnaces
        // → random ticks) is its own sealed contract; the stage wraps it whole.
        self.begin_stage(Stage::WorldScheduled, events);
        self.world.game_tick(&self.recipes);
        self.dispatch_mod_block_hooks(events);
        self.end_stage(Stage::WorldScheduled, events);

        self.begin_stage(Stage::NaturalBreaks, events);
        self.process_natural_breaks(events);
        self.end_stage(Stage::NaturalBreaks, events);

        self.begin_stage(Stage::Pickup, events);
        // Drop lifetime advances once per tick; each player then vacuums
        // eligible drops in session-id order.
        self.world.tick_item_lifetime();
        // Reservations are per-requester: release any whose owner is gone or
        // dead (their pickup pass no longer runs to re-evaluate it), so the
        // drop returns to the pool this tick instead of staying claimed.
        {
            let sessions = &self.sessions;
            self.world
                .dropped_items_mut()
                .release_requests_not_from(|id| {
                    sessions
                        .iter()
                        .any(|sess| sess.id == id && sess.player.health() > 0)
                });
        }
        for s in 0..self.sessions.len() {
            if self.item_pickup_tick(s) {
                events.player(s).picked_up_item = true;
                // Every observer hears the pickup at the collector's body.
                events
                    .world
                    .item_picked_up
                    .push((self.sessions[s].player.body_center(), self.sessions[s].id));
            }
        }
        self.end_stage(Stage::Pickup, events);

        // Player anchors for the entity stages, sampled here (after
        // PlayerDamage teleports settle — same point the old single-player
        // snapshot was taken).
        let anchors: Vec<crate::mob::PlayerAnchor> = self
            .sessions
            .iter()
            .map(|sess| crate::mob::PlayerAnchor {
                id: sess.id,
                pos: sess.player.body_center(),
                // A mounted rider contributes no push body — its body sits
                // inside the mount, and the soft push would shove the mount
                // out from under its own rider every tick.
                body: (!sess.player.is_spectator() && sess.mount.is_none())
                    .then(|| sess.player.body()),
                sneaking: sess.sneaking(),
                held: (!sess.player.is_spectator())
                    .then(|| sess.selected_item())
                    .flatten(),
            })
            .collect();
        // Passive natural spawning still centres on one anchor per tick, round-robin,
        // so its per-tick attempt budget stays constant. Hostile spawning builds its
        // own chunk/cap plan from every connected anchor below.
        let spawn_s = (self.world.current_tick() as usize) % self.sessions.len();

        // Footstep noises for hearing-based mob AI, sampled from the same
        // settled player state as the anchors (block-action noises were pushed
        // by their own funnels during the earlier stages).
        self.push_player_step_noises();

        self.begin_stage(Stage::Mobs, events);
        let mob_events = self.world.tick_mobs(TICK_DT, &anchors);
        self.apply_mob_fall_damage(mob_events.falls, events);
        for splash in mob_events.splashes {
            self.push_water_splash(splash.pos, splash.fall, events);
        }
        // Mob→player combat resolves right after the mobs moved: each strike runs
        // through the engine-owned global i-frame + `player_damage_pre`
        // pipeline, and an applied strike knocks the player back.
        self.apply_mob_attacks(mob_events.attacks, events);
        // Riding shares the stage: dismount valves, detach publication,
        // session-mirror consequences, and slaving every rider to its seat on
        // the mounts' post-tick poses (see `server::riding`).
        self.tick_riding();
        self.end_stage(Stage::Mobs, events);

        self.begin_stage(Stage::ItemPhysics, events);
        // The magnet pulls each requested drop toward ITS requester, so the
        // anchors carry ids alongside the body centres.
        let magnet_anchors: Vec<(PlayerId, crate::mathh::Vec3)> =
            anchors.iter().map(|a| (a.id, a.pos)).collect();
        // Row-declared dropped-item reactions (flour landing in water) return
        // their presentation batch: one burst + sound per transformed ENTITY,
        // routed onto the replicated world-event channels like any other
        // positional one-shot.
        for fx in self.world.tick_item_physics(TICK_DT, &magnet_anchors) {
            if let Some(bundle) = fx.burst {
                events.world.emitter_bursts.push((bundle, fx.pos, 1.0));
            }
            if let Some(sound) = fx.sound {
                events.world.sounds.push(crate::game::ModSound {
                    sound,
                    pos: Some(fx.pos),
                });
            }
        }
        self.end_stage(Stage::ItemPhysics, events);

        self.begin_stage(Stage::Spawning, events);
        // One-time worldgen herds land as chunks near the round-robin player
        // settle; the persisted populated set keeps that stock one-time.
        for (id, kind, pos) in self.world.populate_mobs_tick(anchors[spawn_s].pos) {
            self.bus.emit(PostEvent::MobSpawned { id, kind, pos });
        }
        // The passive trickle backfills on the slow creature cadence — one
        // attempt per player per interval, not per tick, or killing animals
        // becomes a respawn faucet.
        if self.world.current_tick() % crate::mob::PASSIVE_SPAWN_INTERVAL_TICKS == 0 {
            for anchor in &anchors {
                for (id, kind, pos) in self.world.spawn_mobs_tick(anchor.pos) {
                    self.bus.emit(PostEvent::MobSpawned { id, kind, pos });
                }
            }
        }
        self.tick_mod_hostile_mob_spawns(&anchors, events);
        self.end_stage(Stage::Spawning, events);
    }

    /// Forward the behavior hooks the world tick queued on mod-behavior blocks
    /// (see `block::behavior::wasm`) to their owning mods, inside the same
    /// stage window as the world tick that fired them. The queue is drained
    /// unconditionally so it never carries over between ticks.
    fn dispatch_mod_block_hooks(&mut self, events: &mut TickEvents) {
        let hooks = self.world.take_mod_block_hooks();
        if hooks.is_empty() || !self.mods.has_block_behaviors() {
            return;
        }
        let Self {
            world,
            sessions,
            mods,
            bus,
            ..
        } = self;
        Self::with_sessions_view(sessions, 0, |host| {
            let mut ctx = SimCtx {
                world,
                player: &mut host.player,
                gui_state: &mut host.gui_state,
                feed: events,
                queue: bus.queue_mut(),
            };
            mods.dispatch_block_hooks(&mut ctx, &hooks);
        });
    }

    fn tick_mod_hostile_mob_spawns(
        &mut self,
        anchors: &[crate::mob::PlayerAnchor],
        events: &mut TickEvents,
    ) {
        if !self.mods.has_hostile_spawners() {
            return;
        }

        let player_positions: Vec<_> = anchors.iter().map(|a| a.pos).collect();
        let Some(plan) = crate::mob::hostile_spawn_plan(&self.world, &player_positions) else {
            return;
        };

        'attempts: for attempt in 0..crate::mob::HOSTILE_SPAWN_ATTEMPTS {
            let sites = crate::mob::hostile_attempt_sites(&self.world, &plan, attempt);
            for site in sites {
                let kind = {
                    let Self {
                        world,
                        sessions,
                        mods,
                        bus,
                        ..
                    } = self;
                    Self::with_sessions_view(sessions, 0, |host| {
                        let mut ctx = SimCtx {
                            world,
                            player: &mut host.player,
                            gui_state: &mut host.gui_state,
                            feed: events,
                            queue: bus.queue_mut(),
                        };
                        mods.hostile_spawn_kind(&mut ctx, &site.candidate)
                    })
                };
                let Some(kind) = kind else {
                    continue;
                };
                if !crate::mob::hostile_kind_has_room(&self.world, &plan, kind) {
                    continue;
                }
                if let Some(id) = self.world.spawn_mob(kind, site.pos, site.yaw) {
                    self.bus.emit(PostEvent::MobSpawned {
                        id,
                        kind,
                        pos: site.pos,
                    });
                }
                break 'attempts;
            }
        }
    }

    /// Split-borrow `sessions` around the ACTING session and run `f` against
    /// it with the sessions view published: inside `f`, any `SimCtx` built on
    /// the acting session's borrows can reach EVERY connected session's
    /// player through its accessors (`acting_player_id` / `player_ids` /
    /// `with_player`). The acting session is deliberately EXCLUDED from the
    /// published roster — its player is exactly the `&mut` `f` receives, and
    /// the accessors route its id through that borrow, so one player can
    /// never be reachable on two paths (the `with_sessions_scope` soundness
    /// contract). The other sessions' borrows are taken here, before `f`, and
    /// nothing else touches `sessions` until it returns.
    pub(crate) fn with_sessions_view<R>(
        sessions: &mut [ConnectedPlayer],
        acting: usize,
        f: impl FnOnce(&mut ConnectedPlayer) -> R,
    ) -> R {
        let (left, rest) = sessions.split_at_mut(acting);
        let (act, right) = rest.split_first_mut().expect("acting session in range");
        let others: Vec<SessionPlayerRef> = left
            .iter_mut()
            .chain(right.iter_mut())
            .map(|sess| SessionPlayerRef {
                id: sess.id,
                player: &mut sess.player,
            })
            .collect();
        crate::events::with_sessions_scope(act.id, acting, others, || f(act))
    }

    /// Run the systems attached at `at` — the mod seam. A slot with nothing
    /// attached costs one bounds-checked array read per stage edge. The
    /// sessions view rides every run: the HOST session (0) acts, the whole
    /// roster is reachable.
    fn run_systems(&mut self, at: Attach, events: &mut TickEvents) {
        if self.systems.is_empty_at(at) {
            return;
        }
        let Self {
            world,
            sessions,
            systems,
            bus,
            ..
        } = self;
        Self::with_sessions_view(sessions, 0, |host| {
            systems.run(
                at,
                world,
                &mut host.player,
                &mut host.gui_state,
                events,
                bus.queue_mut(),
            );
        });
    }

    /// Open a stage: run its `Before` systems, then apply any mod actions they
    /// queued (`DamagePlayer`/`DamageMob`/... — see `apply_mod_actions`) BEFORE
    /// the engine step runs, so mob indices captured by those systems cannot be
    /// shifted by the step in between.
    fn begin_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::Before(stage), events);
        self.apply_mod_actions(events);
    }

    /// Close a stage: run its `After` systems, apply the mod actions they (or
    /// the stage's inline pre-event handlers) queued, then drain the post
    /// queue — so post events emitted by those actions (`player_damaged`,
    /// `mob_died`) dispatch within the same tick, at the earliest defined
    /// point. Actions queued by post handlers during the drain roll to the
    /// next action point (next stage or next tick's start) — no recursion.
    fn end_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::After(stage), events);
        self.apply_mod_actions(events);
        self.drain_post_events(events);
    }

    fn drain_post_events(&mut self, events: &mut TickEvents) {
        if !self.bus.has_queued_posts() {
            return;
        }
        let Self {
            world,
            sessions,
            bus,
            ..
        } = self;
        Self::with_sessions_view(sessions, 0, |host| {
            bus.drain_post(world, &mut host.player, &mut host.gui_state, events);
        });
    }
}
