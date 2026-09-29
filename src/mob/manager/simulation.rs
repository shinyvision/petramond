use crate::mob::anim::Expression;
use crate::mob::brain::ScriptedReplies;
use crate::mob::brain::{AiMob, TickInputs};
use crate::mob::instance::{Begun, Footing, MobTickCtx, MotionStart, SpeciesMeta};
use crate::mob::model_meta;
use crate::mob::noise::{Noise, NoiseKind};
use crate::mob::{def, defs, model, EntityRef, Instance, Mob};
use crate::world::ServerWorld;
use petramond_math::math::Vec3;
use petramond_world::body::Body;

use super::{lod, nearest_anchor, Mobs};

#[derive(Copy, Clone, Debug)]
pub struct PlayerAnchor {
    pub id: crate::player::PlayerId,
    pub pos: petramond_math::world_pos::WorldPos,
    pub body: Option<Body>,
    pub sneaking: bool,
    pub held: Option<petramond_world::item::ItemType>,
}

impl Default for PlayerAnchor {
    fn default() -> Self {
        PlayerAnchor {
            id: Default::default(),
            pos: petramond_math::world_pos::WorldPos::ZERO,
            body: None,
            sneaking: false,
            held: None,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MobAttack {
    pub mob: Mob,
    pub mob_id: u64,
    pub origin: petramond_math::world_pos::WorldPos,
    pub target: EntityRef,
    pub damage: f32,
    pub knockback_dir: Vec3,
    pub knockback: f32,
}

#[derive(Copy, Clone, Debug)]
pub struct MobFall {
    pub mob_id: u64,
    pub distance: f32,
}

#[derive(Copy, Clone, Debug)]
pub struct MobSplash {
    pub pos: petramond_math::world_pos::WorldPos,
    pub fall: f32,
}

#[derive(Copy, Clone, Debug)]
pub struct MobExposureDamage {
    pub mob_id: u64,
    pub damage: petramond_world::exposure::ExposureDamage,
}

#[derive(Default, Debug)]
pub struct MobTickEvents {
    pub attacks: Vec<MobAttack>,
    pub falls: Vec<MobFall>,
    pub splashes: Vec<MobSplash>,
    pub exposure: Vec<MobExposureDamage>,
}

pub(super) struct Turn {
    begun: Option<Begun>,
    footing: Option<Footing>,
    requests: std::ops::Range<usize>,
    pub(super) start: MotionStart,
}

impl Default for Turn {
    fn default() -> Self {
        Self {
            begun: None,
            footing: None,
            requests: 0..0,
            start: None,
        }
    }
}

impl Turn {
    pub(super) fn simulated(&self) -> bool {
        self.begun.is_some()
    }
}

type MobMeta = SpeciesMeta;

static MOB_META: petramond_world::content::Slot<Vec<MobMeta>> =
    petramond_world::content::Slot::new("mob model metadata", &["mobs.json"], derive_meta);

fn derive_meta(_: &petramond_world::content::ContentRegistry) -> Result<Vec<MobMeta>, String> {
    Ok(defs()
        .iter()
        .map(|d| SpeciesMeta {
            idle_anims: model_meta::idle_anims(model(d.mob)),
            named_anims: model_meta::named_anims(model(d.mob)),
            skeleton: model_meta::skeleton(model(d.mob)),
        })
        .collect())
}

impl Mobs {
    /// One tick for every mob: species idle-anim + ragdoll skeleton, cached skylight, soft pushing,
    /// then drop mobs that should leave (finished corpse, or a culled distance-despawned hostile -
    /// not saved). Returns melee strikes (player damage) and landed falls (mob damage) from this
    /// tick.
    ///
    /// `player_pos` is the body centre, used by AI for head-look and despawn distance.
    /// `player_body` is the pushable body, only set when the player has a physical presence (not
    /// noclip) - when set, mobs get shoved off it this tick. We don't push the player back here:
    /// that moves the player and needs per-frame smoothness, so the caller does it via
    /// [`push_on_player`](Self::push_on_player).
    ///
    /// With `freeze_unloaded` on (save attached), a mob over a not-yet-loaded chunk is frozen and
    /// skipped from pushing, until unload harvests it into that chunk's record. Same idea as the
    /// dropped-item freeze - keeps mobs from falling through missing terrain at the streaming edge.
    ///
    /// [Sim distance](super::SimDistance) controls how much of a mob's tick runs: full near a
    /// player, reduced-rate brain farther out, just despawn checks beyond that (see `lod`).
    pub fn tick(
        &mut self,
        dt: f32,
        world: &ServerWorld,
        anchors: &[PlayerAnchor],
        freeze_unloaded: bool,
    ) -> MobTickEvents {
        let mut grounded = std::mem::take(&mut self.grounded_scratch);
        grounded.clear();
        grounded.extend(
            self.list
                .iter()
                .map(|m| !freeze_unloaded || terrain_under_mob_is_final(world, m)),
        );
        let mut ai_mobs = std::mem::take(&mut self.ai_snapshot);
        ai_mobs.rebuild(self.list.iter().zip(&grounded).map(|(m, &grounded)| AiMob {
            id: m.id(),
            kind: m.kind,
            pos: m.pos,
            active: !m.is_dead() && grounded,
            tags: m.tags_shared(),
        }));
        let solid = self.solid_obstacles();
        self.heard.take_batch(&mut self.pending_noises);
        let mut pending_noises = std::mem::take(&mut self.pending_noises);
        let mut turns = std::mem::take(&mut self.turns);
        turns.clear();
        turns.resize_with(self.list.len(), Turn::default);
        self.confined_regions.set_now(world.current_tick());
        let mut confined_regions = std::mem::take(&mut self.confined_regions);

        let now = world.current_tick();
        let mut steps = std::mem::take(&mut self.step_scratch);
        steps.clear();
        steps.extend(
            self.list
                .iter()
                .map(|m| self.sim_distance.step(m, anchors, now)),
        );
        self.path_budget.refill(
            self.list
                .iter()
                .zip(&steps)
                .filter(|(m, step)| **step == lod::SimStep::Think && m.nav_search_waiting())
                .count(),
        );

        for (i, mob) in self.list.iter_mut().enumerate() {
            if !grounded[i] {
                mob.clear_drive();
                continue;
            }
            let d = def(mob.kind);
            let anchor = *nearest_anchor(anchors, mob.pos);
            if steps[i] == lod::SimStep::Frozen {
                mob.tick_frozen(anchor.pos, d.despawn_radius);
                continue;
            }
            let begun = mob.begin(dt, anchor.pos, d.despawn_radius);
            turns[i].begun = Some(begun);
            if begun == Begun::Corpse {
                let meta = &MOB_META.current()[mob.kind.0 as usize];
                mob.tick_ragdoll(dt, world, d, &meta.skeleton);
            } else if begun == Begun::Placed {
                let meta = &MOB_META.current()[mob.kind.0 as usize];
                mob.apply_expression(dt, d, &meta.named_anims, &Expression::default());
            }
        }

        let mut supports = std::mem::take(&mut self.solid.supports);
        for (i, mob) in self.list.iter_mut().enumerate() {
            if turns[i].begun != Some(Begun::Live) {
                continue;
            }
            let d = def(mob.kind);
            let meta = &MOB_META.current()[mob.kind.0 as usize];
            let anchor = *nearest_anchor(anchors, mob.pos);
            let peer_obstacles = if d.collision == super::MobCollision::Solid {
                supports.clear();
                super::append_body_supports(
                    mob.pos,
                    mob.yaw,
                    d.size,
                    &solid,
                    mob.id(),
                    &mut supports,
                );
                supports.as_slice()
            } else {
                solid.as_slice()
            };
            let inputs = TickInputs {
                world,
                players: anchors,
                noises: &self.heard,
                mobs: &ai_mobs,
                path_budget: Some(&self.path_budget),
                solid: peer_obstacles,
                solid_escape: &solid,
            };
            let ctx = MobTickCtx {
                dt,
                inputs: &inputs,
                anchor: &anchor,
                def: d,
                meta,
            };
            turns[i].footing =
                Some(mob.perceive(&ctx, &mut confined_regions, steps[i] == lod::SimStep::Think));
        }

        let mut requests = std::mem::take(&mut self.scripted_requests);
        requests.clear();
        for (i, mob) in self.list.iter_mut().enumerate() {
            let Some(footing) = turns[i].footing else {
                continue;
            };
            if steps[i] != lod::SimStep::Think {
                continue;
            }
            let d = def(mob.kind);
            let meta = &MOB_META.current()[mob.kind.0 as usize];
            let anchor = *nearest_anchor(anchors, mob.pos);
            let inputs = TickInputs {
                world,
                players: anchors,
                noises: &self.heard,
                mobs: &ai_mobs,
                path_budget: Some(&self.path_budget),
                solid: &solid,
                solid_escape: &solid,
            };
            let ctx = MobTickCtx {
                dt,
                inputs: &inputs,
                anchor: &anchor,
                def: d,
                meta,
            };
            let start = requests.len();
            mob.scripted_requests(&ctx, footing, &mut requests);
            turns[i].requests = start..requests.len();
        }
        let mut replies = crate::modding::ai::dispatch_batch(now, &requests);
        requests.clear();
        self.scripted_requests = requests;

        for (i, mob) in self.list.iter_mut().enumerate() {
            let Some(footing) = turns[i].footing else {
                continue;
            };
            let d = def(mob.kind);
            let meta = &MOB_META.current()[mob.kind.0 as usize];
            let anchor = *nearest_anchor(anchors, mob.pos);
            let peer_obstacles = if d.collision == super::MobCollision::Solid {
                supports.clear();
                super::append_body_supports(
                    mob.pos,
                    mob.yaw,
                    d.size,
                    &solid,
                    mob.id(),
                    &mut supports,
                );
                supports.as_slice()
            } else {
                solid.as_slice()
            };
            let inputs = TickInputs {
                world,
                players: anchors,
                noises: &self.heard,
                mobs: &ai_mobs,
                path_budget: Some(&self.path_budget),
                solid: peer_obstacles,
                solid_escape: &solid,
            };
            let ctx = MobTickCtx {
                dt,
                inputs: &inputs,
                anchor: &anchor,
                def: d,
                meta,
            };
            let think = steps[i] == lod::SimStep::Think;
            let range = turns[i].requests.clone();
            let decision = mob.think(
                &ctx,
                footing,
                think,
                ScriptedReplies::new(&mut replies[range]),
            );
            turns[i].start = mob.act(&ctx, footing, &decision, think);
            mob.apply_expression(dt, d, &meta.named_anims, &decision.into());
        }
        self.solid.supports = supports;
        self.confined_regions = confined_regions;
        self.step_scratch = steps;
        self.grounded_scratch = grounded;
        self.turns = turns;
        self.solve_solids(world, solid);
        let turns = std::mem::take(&mut self.turns);

        let mut out = MobTickEvents::default();
        let mut exposure = std::mem::take(&mut self.exposure_scratch);
        for (i, mob) in self.list.iter_mut().enumerate() {
            if !turns[i].simulated() {
                continue;
            }
            if let Some((was_on_ground, _)) = turns[i].start {
                let d = super::super::def(mob.kind);
                let immersion = world.data().body_fluid(mob.pos, d.size.height, d.buoyancy);
                mob.finish_motion(was_on_ground, immersion);
            }
            if mob.moving && def(mob.kind).step_noise {
                pending_noises.push(Noise {
                    pos: mob.pos,
                    kind: NoiseKind::Step,
                    source: EntityRef::Mob(mob.id()),
                });
            }
            if let Some(intent) = mob.take_attack() {
                let target_pos = match intent.target {
                    EntityRef::Player(pid) => anchors.iter().find(|a| a.id == pid).map(|a| a.pos),
                    EntityRef::Mob(id) => ai_mobs.live(id).map(|m| m.pos),
                };
                if let Some(target_pos) = target_pos {
                    let mut away = target_pos - mob.pos;
                    away.y = 0.0;
                    out.attacks.push(MobAttack {
                        mob: mob.kind,
                        mob_id: mob.id(),
                        origin: mob.pos,
                        target: intent.target,
                        damage: intent.damage,
                        knockback_dir: away.normalize_or_zero(),
                        knockback: intent.knockback,
                    });
                }
            }
            if let Some(distance) = mob.take_fall_distance() {
                out.falls.push(MobFall {
                    mob_id: mob.id(),
                    distance,
                });
            }
            if let Some(fall) = mob.take_splash_drop() {
                out.splashes.push(MobSplash { pos: mob.pos, fall });
            }
            if !mob.is_dead() {
                let d = def(mob.kind);
                let boxes = crate::mob::body_geometry::body_boxes(mob.pos, mob.yaw, d.size);
                exposure.tick(world, boxes, mob.exposure_mut());
                let mob_id = mob.id();
                out.exposure.extend(
                    exposure
                        .damage
                        .drain(..)
                        .map(|damage| MobExposureDamage { mob_id, damage }),
                );
            }
            let c = (mob.pos + Vec3::new(0.0, 0.3, 0.0)).block();
            mob.skylight = world.data().skylight6_at_world(c.x, c.y, c.z);
            mob.blocklight = petramond_world::light::BlockLight6::from_x2(
                world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
            );
        }
        self.turns = turns;
        self.exposure_scratch = exposure;

        self.pending_noises = pending_noises;
        self.ai_snapshot = ai_mobs;
        self.resolve_pushes(anchors);
        for i in 0..self.list.len() {
            if self.list[i].is_distance_despawned() {
                self.spill_container(i);
            }
        }
        self.retain_instances(|m| !m.is_despawned() && !m.is_distance_despawned());
        out
    }
}

fn terrain_under_mob_is_final(world: &ServerWorld, mob: &Instance) -> bool {
    let c = mob.pos.block();
    c.y >= petramond_world::chunk::WORLD_MIN_Y
        && world.physics_cell_final_at(c.x, c.y, c.z)
        && world.physics_cell_final_at(c.x, c.y - 1, c.z)
}
