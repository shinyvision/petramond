use crate::events::tick::{TickEvents, TICK_DT};
use crate::events::{Attach, PostEvent, Stage};

use super::ServerGame;

impl ServerGame {
    pub fn run_fixed_ticks(&mut self, dt: f32) -> (TickEvents, u32) {
        let due = self.clock.due_ticks(dt);
        let mut events = self.mods.open_feed();
        for _ in 0..due {
            self.game_tick_step(&mut events);
        }
        self.mods.settle_feed(&events);
        (events, due)
    }

    pub fn game_tick_step(&mut self, events: &mut TickEvents) {
        self.tick_damage_immunity();
        self.settle_before_stages(events);
        self.player_stages(events);
        self.world_stages(events);
        self.pickup_stage(events);
        let anchors = self.player_anchors();
        self.entity_stages(&anchors, events);
    }

    fn settle_before_stages(&mut self, events: &mut TickEvents) {
        super::stream_events::pump_stream_events(&mut self.world, &mut self.mods);
        self.publish_dismounted();
        self.apply_deferred_actions(events);
        self.containers
            .release_absent_holders(self.world.mobs(), events);
        self.drain_post_events(events);
    }

    fn player_stages(&mut self, events: &mut TickEvents) {
        self.publish_player_inputs();
        for s in 0..self.sessions.len() {
            let gameplay = self.sessions[s].input.intent_gameplay;
            self.sessions[s].player.refresh_engine_claims(gameplay);
        }
        crate::server::movement::tick_movements(&self.world, &mut self.sessions);

        self.begin_stage(Stage::Mining, events);
        self.for_each_session("mining", events, |server, s, events| {
            server.tick_mining(s, events);
        });
        self.end_stage(Stage::Mining, events);

        self.begin_stage(Stage::Placement, events);
        self.for_each_session("placement", events, |server, s, events| {
            server.tick_place(s, events);
            server.tick_creative(s, events);
        });
        self.tick_schematics();
        self.end_stage(Stage::Placement, events);

        self.begin_stage(Stage::Attack, events);
        self.for_each_session("attack", events, |server, s, events| {
            server.tick_attack(s, events);
        });
        self.end_stage(Stage::Attack, events);

        self.begin_stage(Stage::Drops, events);
        self.for_each_session("drops", events, |server, s, events| {
            server.tick_drops(s, events);
        });
        self.end_stage(Stage::Drops, events);

        self.begin_stage(Stage::Menu, events);
        self.close_menus_on_absent_anchors(events);
        self.for_each_session("menu", events, |server, s, events| {
            server.tick_menu(s, events);
        });
        self.end_stage(Stage::Menu, events);

        self.begin_stage(Stage::PlayerDamage, events);
        self.for_each_session("player damage", events, |server, s, events| {
            server.tick_fall_damage(s, events);
            server.tick_player_exposure(s, events);
            server.tick_fluid_splash(s, events);
            server.sessions[s].tick_effects();
            server.tick_bed_and_respawn(s, events);
        });
        self.resolve_sleep_completion(events);
        self.end_stage(Stage::PlayerDamage, events);
    }

    fn world_stages(&mut self, events: &mut TickEvents) {
        self.begin_stage(Stage::WorldScheduled, events);
        self.world.game_tick(self.catalog.recipes());
        self.dispatch_block_hooks(events);
        self.bake_dirty_custom_shapes(events);
        self.end_stage(Stage::WorldScheduled, events);

        self.begin_stage(Stage::NaturalBreaks, events);
        self.process_natural_breaks(events);
        self.end_stage(Stage::NaturalBreaks, events);
    }

    fn pickup_stage(&mut self, events: &mut TickEvents) {
        self.begin_stage(Stage::Pickup, events);
        if self
            .world
            .current_tick()
            .is_multiple_of(u64::from(crate::world::ITEM_MERGE_INTERVAL_TICKS))
        {
            self.world.dropped_items_mut().merge_nearby();
        }
        self.world.tick_item_lifetime();
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
        self.for_each_session("pickup", events, |server, s, events| {
            if server.item_pickup_tick(s) {
                events.player(s).picked_up_item = true;
                events.world.item_picked_up.push((
                    server.sessions[s].player.body_center(),
                    server.sessions[s].id,
                ));
            }
        });
        self.end_stage(Stage::Pickup, events);
    }

    fn player_anchors(&self) -> Vec<crate::mob::PlayerAnchor> {
        self.sessions
            .iter()
            .map(|sess| crate::mob::PlayerAnchor {
                id: sess.id,
                pos: sess.player.body_center(),
                body: (!sess.player.is_spectator() && sess.sim.mount.is_none())
                    .then(|| sess.player.body()),
                sneaking: sess.sneaking(),
                held: (!sess.player.is_spectator())
                    .then(|| sess.selected_item())
                    .flatten(),
            })
            .collect()
    }

    fn entity_stages(&mut self, anchors: &[crate::mob::PlayerAnchor], events: &mut TickEvents) {
        let spawn_anchor = anchors
            .get((self.world.current_tick() as usize) % anchors.len().max(1))
            .map(|anchor| anchor.pos);

        crate::server::entities::push_player_step_noises(&mut self.world, &self.sessions);

        self.begin_stage(Stage::Mobs, events);
        let mob_events = self.world.tick_mobs(TICK_DT, anchors);
        self.apply_mob_fall_damage(mob_events.falls, events);
        self.apply_mob_exposure_damage(mob_events.exposure, events);
        for splash in mob_events.splashes {
            self.push_fluid_splash(splash.pos, splash.fall, events);
        }
        self.apply_mob_attacks(mob_events.attacks, events);
        self.tick_riding();
        self.end_stage(Stage::Mobs, events);

        self.begin_stage(Stage::ItemPhysics, events);
        let step = self.world.tick_item_physics(TICK_DT, anchors);
        for fx in step.fx {
            if let Some(bundle) = fx.burst {
                events
                    .world
                    .emitter_bursts
                    .push(crate::events::tick::BurstFired::plain(bundle, fx.pos, 1.0));
            }
            if let Some(sound) = fx.sound {
                events.world.sounds.push(crate::events::tick::SoundEvent {
                    sound,
                    pos: Some(fx.pos),
                });
            }
        }
        self.resolve_item_impacts(step.impacts, events);
        self.end_stage(Stage::ItemPhysics, events);

        self.begin_stage(Stage::Spawning, events);
        if let Some(anchor) = spawn_anchor {
            for (id, kind, pos) in self.world.populate_mobs_tick(anchor) {
                self.mods.emit(PostEvent::MobSpawned { id, kind, pos });
            }
        }
        if self
            .world
            .current_tick()
            .is_multiple_of(crate::mob::PASSIVE_SPAWN_INTERVAL_TICKS)
        {
            for anchor in anchors {
                for (id, kind, pos) in self.world.spawn_mobs_tick(anchor.pos) {
                    self.mods.emit(PostEvent::MobSpawned { id, kind, pos });
                }
            }
        }
        self.tick_hostile_mob_spawns(anchors, events);
        crate::server::progression::detect_obtained_items(&mut self.sessions, &mut self.mods);
        self.end_stage(Stage::Spawning, events);
    }

    fn dispatch_block_hooks(&mut self, events: &mut TickEvents) {
        let hooks = self.world.take_block_hooks();
        if hooks.is_empty() || !self.mods.host().has_block_behaviors() {
            return;
        }
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.dispatch(world, sessions, None, events, |host, ctx| {
            host.dispatch_block_hooks(ctx, &hooks)
        });
    }

    fn bake_dirty_custom_shapes(&mut self, events: &mut TickEvents) {
        if !self.world.data().has_pending_custom_bakes() {
            return;
        }
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.dispatch(world, sessions, None, events, |host, ctx| {
            host.bake_custom_shapes(ctx)
        });
    }

    fn tick_hostile_mob_spawns(
        &mut self,
        anchors: &[crate::mob::PlayerAnchor],
        events: &mut TickEvents,
    ) {
        if !self.mods.host().has_hostile_spawners() {
            return;
        }

        let player_positions: Vec<_> = anchors.iter().map(|a| a.pos).collect();
        let Some(plan) = crate::mob::hostile_spawn_plan(
            &self.world,
            &mut self.hostile_spawn_cache,
            &player_positions,
        ) else {
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
                        ..
                    } = self;
                    mods.dispatch(world, sessions, None, events, |host, ctx| {
                        host.hostile_spawn_kind(ctx, &site.candidate)
                    })
                };
                let Some(kind) = kind else {
                    continue;
                };
                if !crate::mob::hostile_kind_has_room(&self.world, &plan, kind) {
                    continue;
                }
                if let Some(id) = self.world.spawn_mob(kind, site.pos, site.yaw) {
                    self.mods.emit(PostEvent::MobSpawned {
                        id,
                        kind,
                        pos: site.pos,
                    });
                }
                break 'attempts;
            }
        }
    }

    fn run_systems(&mut self, at: Attach, events: &mut TickEvents) {
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.run_systems(at, world, sessions, events);
    }

    fn begin_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::Before(stage), events);
        self.apply_deferred_actions(events);
    }

    fn end_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::After(stage), events);
        self.apply_deferred_actions(events);
        self.scatter_mob_spills();
        self.drain_post_events(events);
    }

    pub fn drain_post_events(&mut self, events: &mut TickEvents) {
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.drain_posts(world, sessions, events);
    }
}
