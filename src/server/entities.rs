use crate::entity::DroppedItem;
use crate::events::{DamageSource, MobDamagePre, Outcome, PostEvent};
use crate::mob::{def as mob_def, DeathDrop, MobAttack, MobDamageSound, MobFall, MobSoundCategory};
use petramond_math::math::Vec3;

const SPLASH_MIN_FALL: f32 = 1.5;
const SPLASH_BIG_FALL: f32 = 5.0;
use crate::world::ServerWorld;

use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::server::health::fall_damage_health;

pub(super) const MOB_ATTACK_UP_RATIO: f32 = 0.65;

impl ServerGame {
    /// Every source of mob damage goes through here. We bail if the victim is immune, fire
    /// `mob_damage_pre` (mods can change or cancel the amount), apply the rest via
    /// [`Mobs::damage_mob`](crate::mob::Mobs::damage_mob), and on a kill queue `mob_died` and roll
    /// loot. Returns whether anything was applied.
    ///
    /// `feedback` is this request's pipeline; `None` means the species' `damage_feedback`. Without
    /// the `Immunity` component it's DoT (burn ticks): i-frames don't block it and it doesn't
    /// start any.
    ///
    /// `mob_damage_pre` runs as the attacking player when the source names one, and actor-less
    /// otherwise (a mob's bite, a fall, mod damage).
    pub fn damage_mob_through_pipeline(
        &mut self,
        mob_id: crate::mob::MobId,
        amount: f32,
        source: DamageSource,
        origin: Option<petramond_math::world_pos::WorldPos>,
        feedback: Option<crate::mob::MobDamageFeedback>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(snapshot) = self
            .world
            .mobs()
            .get(mob_id)
            .map(|m| (m.kind, m.pos, m.is_dead(), m.is_damage_immune()))
        else {
            return false;
        };
        let (kind, pos, was_dead, damage_immune) = snapshot;
        let mut feedback = feedback.unwrap_or_else(|| mob_def(kind).damage_feedback.clone());
        let weapon = self.weapon_knockback(source);
        if weapon != 1.0 {
            for component in &mut feedback.components {
                if let crate::mob::MobDamageFeedbackComponent::Knockback { scale, .. } = component {
                    *scale *= weapon;
                }
            }
        }
        if was_dead || (damage_immune && feedback.has_immunity()) {
            return false;
        }
        let mut pre = MobDamagePre {
            mob_id,
            kind,
            amount,
            source,
            origin,
            feedback,
        };
        let actor = source.attacker().and_then(crate::mob::EntityRef::player);
        let cancelled = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let bus = mods.bus_mut();
            bus.mob_damage_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel
        };
        if cancelled {
            return false;
        }
        if !pre.feedback.has_any_component() {
            return false;
        }
        let soundable_hit = pre.feedback.plays_sound(MobDamageSound::Hurt) && pre.amount > 0.0;
        let death = self.world.mobs_mut().damage_mob(
            mob_id,
            pre.amount,
            pre.origin,
            pre.source.is_attack(),
            pre.source.attacker(),
            &pre.feedback,
        );
        self.mods.emit(PostEvent::MobDamaged {
            mob_id,
            kind,
            amount: pre.amount,
            source: pre.source,
            killed: death.is_some(),
        });
        if let Some(death) = death {
            if pre.feedback.plays_sound(MobDamageSound::Death) {
                queue_mob_sound(events, mob_id, kind, MobSoundCategory::Death, death.pos);
            }
            self.mods.emit(PostEvent::MobDied {
                id: mob_id,
                kind: death.kind,
                pos: death.pos,
            });
            self.spawn_mob_loot(death);
        } else if soundable_hit {
            queue_mob_sound(events, mob_id, kind, MobSoundCategory::Hurt, pos);
        }
        true
    }

    pub fn apply_mob_attacks(&mut self, attacks: Vec<MobAttack>, events: &mut TickEvents) {
        for a in attacks {
            match a.target {
                crate::mob::EntityRef::Player(pid) => {
                    let Some(s) = self.sessions.index_of(pid) else {
                        continue;
                    };
                    if self.sessions[s].player.is_spectator() {
                        continue;
                    }
                    let amount = a.damage.max(0.0).round() as i32;
                    let source = DamageSource::MobAttack {
                        kind: a.mob,
                        id: a.mob_id,
                    };
                    if self.damage_player(s, amount, source, Some(a.origin), events) {
                        let impulse = a.knockback_dir * a.knockback
                            + Vec3::new(0.0, a.knockback * MOB_ATTACK_UP_RATIO, 0.0);
                        self.sessions[s].player.apply_knockback(impulse);
                    }
                }
                crate::mob::EntityRef::Mob(target_id) => {
                    self.damage_mob_through_pipeline(
                        target_id,
                        a.damage.max(0.0),
                        DamageSource::MobAttack {
                            kind: a.mob,
                            id: a.mob_id,
                        },
                        Some(a.origin),
                        None,
                        events,
                    );
                }
            }
        }
    }

    pub fn apply_mob_fall_damage(&mut self, falls: Vec<MobFall>, events: &mut TickEvents) {
        for fall in falls {
            let amount = fall_damage_health(fall.distance) as f32;
            if amount <= 0.0 {
                continue;
            }
            self.damage_mob_through_pipeline(
                fall.mob_id,
                amount,
                DamageSource::Fall,
                None,
                None,
                events,
            );
        }
    }

    pub fn push_fluid_splash(
        &mut self,
        feet: petramond_math::world_pos::WorldPos,
        fall: f32,
        events: &mut TickEvents,
    ) {
        if fall < SPLASH_MIN_FALL {
            return;
        }
        let cell = feet.block();
        let Some(surface) = self.world.data().fluid_surface_at(cell).or_else(|| {
            self.world
                .data()
                .fluid_surface_at(cell - petramond_math::math::IVec3::Y)
        }) else {
            return;
        };
        let Some(splash) = surface.fluid.splash else {
            return;
        };
        let pos = petramond_math::world_pos::WorldPos::new(
            feet.x,
            f64::from(surface.surface_y.floor() + 1.02),
            feet.z,
        );
        events
            .world
            .emitter_bursts
            .push(crate::events::tick::BurstFired::plain(
                splash.burst,
                pos,
                fall,
            ));
        let sound = if fall >= SPLASH_BIG_FALL {
            splash.sound_big
        } else {
            splash.sound_small
        };
        events.world.sounds.push(crate::events::tick::SoundEvent {
            sound,
            pos: Some(pos),
        });
    }

    pub fn push_block_noise(
        &mut self,
        s: usize,
        pos: petramond_math::math::IVec3,
        kind: crate::mob::NoiseKind,
    ) {
        self.push_noise_from(
            crate::mob::EntityRef::Player(self.sessions[s].id),
            pos,
            kind,
        );
    }

    pub fn push_noise_from(
        &mut self,
        source: crate::mob::EntityRef,
        pos: petramond_math::math::IVec3,
        kind: crate::mob::NoiseKind,
    ) {
        self.world.push_noise(crate::mob::Noise {
            pos: petramond_math::world_pos::WorldPos::block_center(pos),
            kind,
            source,
        });
    }

    pub fn scatter_mob_spills(&mut self) {
        for spill in self.world.mobs_mut().take_spills() {
            let centre = spill.pos + Vec3::new(0.0, 0.3, 0.0);
            for stack in spill.stacks {
                let mut drop = DroppedItem::new(centre, stack, self.seeds.draw());
                drop.skylight = spill.skylight;
                drop.blocklight = spill.blocklight;
                self.world.spawn_item(drop);
            }
        }
    }

    pub fn spawn_mob_loot(&mut self, death: DeathDrop) {
        let Some(table) = crate::mob::def(death.kind).loot.as_deref() else {
            return;
        };
        let mut rng = crate::mob::MobRng::new(self.seeds.draw() as u64);
        let stacks = petramond_world::loot::catalog()
            .roll(table, || rng.next_u64())
            .unwrap_or_default();
        let centre = death.pos + Vec3::new(0.0, 0.3, 0.0);
        for stack in stacks {
            let mut drop = DroppedItem::new(centre, stack, self.seeds.draw());
            drop.skylight = death.skylight;
            drop.blocklight = death.blocklight;
            self.world.spawn_item(drop);
        }
    }

    pub fn item_pickup_tick(&mut self, s: usize) -> bool {
        if self.sessions[s].player.health() == 0 {
            return false;
        }
        let requester = self.sessions[s].id;
        let player_pos = self.sessions[s].player.body_center();
        let mut planned = self.sessions[s].player.inventory.clone();
        self.world
            .dropped_items_mut()
            .request_pickups(requester, player_pos, |stack| {
                let count = planned.pickup_fits_count(stack);
                if count > 0 {
                    let leftover = planned.pickup(stack.restack(count));
                    debug_assert!(
                        leftover.is_none(),
                        "pickup_fits_count overestimated pickup capacity"
                    );
                }
                count
            });

        let inventory = &mut self.sessions[s].player.inventory;
        let mut collected = Vec::new();
        self.world
            .dropped_items_mut()
            .collect_requested_pickups(requester, player_pos, |stack| {
                collected.push(stack);
                inventory.pickup(stack)
            });
        let picked_up = !collected.is_empty();
        for stack in collected {
            self.mods.emit(PostEvent::ItemPickedUp {
                player: requester,
                item: stack.item,
                count: stack.count,
                pos: player_pos,
            });
        }
        picked_up
    }
}

fn queue_mob_sound(
    events: &mut TickEvents,
    mob_id: u64,
    kind: crate::mob::Mob,
    category: MobSoundCategory,
    pos: petramond_math::world_pos::WorldPos,
) {
    if crate::mob::def(kind).sound_for(category).is_some() {
        events
            .world
            .mob_sounds
            .push(crate::events::tick::MobSoundEvent {
                mob_id,
                kind,
                category,
                pos,
            });
    }
}

pub fn light_at_pos(
    world: &ServerWorld,
    pos: petramond_math::world_pos::WorldPos,
) -> (u8, petramond_world::light::BlockLight6) {
    let c = pos.block();
    world.data().dynamic_light_at_world(c.x, c.y, c.z)
}

pub(in crate::server) fn push_player_step_noises(
    world: &mut ServerWorld,
    sessions: &crate::server::sessions::SessionRegistry,
) {
    for sess in sessions {
        let p = &sess.player;
        let horizontal_sq = p.vel.x * p.vel.x + p.vel.z * p.vel.z;
        if crate::mob::player_steps_are_audible(
            horizontal_sq,
            p.on_ground,
            sess.sneaking(),
            p.is_spectator(),
        ) {
            world.push_noise(crate::mob::Noise {
                pos: p.pos,
                kind: crate::mob::NoiseKind::Step,
                source: crate::mob::EntityRef::Player(sess.id),
            });
        }
    }
}
