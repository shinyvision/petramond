
use crate::mob::brain::{AiMob, TickInputs};
use crate::mob::model_meta;
use crate::mob::instance::{Begun, Footing, MobTickCtx, MotionStart, SpeciesMeta};
use crate::mob::brain::ScriptedReplies;
use crate::mob::anim::Expression;
use crate::mob::noise::{Noise, NoiseKind};
use crate::mob::{def, defs, model, EntityRef, Instance, Mob};
use crate::world::ServerWorld;
use petramond_math::math::Vec3;
use petramond_world::body::Body;

use super::{lod, nearest_anchor, Mobs};

/// One player's presence as the mob simulation sees it: an AI/despawn anchor
/// plus (for non-spectators) a pushable body. The mobs target whichever anchor
/// is NEAREST per mob, so N players share one world of mobs.
#[derive(Copy, Clone, Debug)]
pub struct PlayerAnchor {
    pub id: crate::player::PlayerId,
    /// Body centre — the AI's target/despawn anchor (matches the old single
    /// `player_pos` argument).
    pub pos: petramond_math::world_pos::WorldPos,
    /// The pushable body; `None` for a spectator (nothing to jostle or strike).
    pub body: Option<Body>,
    /// Whether this player is sneaking — hostile detection shrinks for a
    /// sneaking target (`chase_player`'s `sneak_radius_penalty`).
    pub sneaking: bool,
    /// The player's selected (held) item — the visible-to-the-world hand
    /// fact behaviors may react to (a wheat lure). `None` for an empty hand
    /// or a spectator (who shows nothing to the world).
    pub held: Option<petramond_world::item::ItemType>,
}

/// A neutral anchor (player 0 at the origin, bodiless, empty-handed) — the
/// base tests override per field, so a new perception fact costs one field
/// here instead of a struct-literal edit at every anchor site.
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

/// A melee strike a mob landed this tick. Drained from [`Mobs::tick`] by
/// `Game`, which applies it through the target's damage pipeline: a player
/// target runs `player_damage_pre` (a cancelled strike drops its knockback
/// too); a mob target runs the shared mob damage pipeline (`mob_damage_pre`,
/// feedback, loot, ragdoll).
#[derive(Copy, Clone, Debug)]
pub struct MobAttack {
    pub mob: Mob,
    /// The attacker's STABLE id — carried into the mob damage pipeline so the
    /// struck mob's retaliation memory can name the biter across ticks.
    pub mob_id: u64,
    /// Attacker position for damage origin / presentation context.
    pub origin: petramond_math::world_pos::WorldPos,
    /// Who the strike lands on (whatever the attacker's brain locked).
    pub target: EntityRef,
    /// Damage in half-heart points (rounded when applied to a player).
    pub damage: f32,
    /// Horizontal unit direction the target is knocked toward (away from the mob;
    /// zero when the two exactly overlap — the strike still pops upward).
    pub knockback_dir: Vec3,
    /// Horizontal knockback speed (m/s) added to a player target's velocity.
    /// A mob target takes its own row's knockback feedback instead.
    pub knockback: f32,
}

/// A fall landing measured by a mob during its deterministic tick, addressed by the
/// mob's stable id for `ServerGame` to apply through the mob damage pipeline.
#[derive(Copy, Clone, Debug)]
pub struct MobFall {
    pub mob_id: u64,
    pub distance: f32,
}

/// A mob fell into a splashing fluid this tick (its un-latched fall drop at the first wet
/// tick). `ServerGame` throws the splash burst above the entry point.
#[derive(Copy, Clone, Debug)]
pub struct MobSplash {
    pub pos: petramond_math::world_pos::WorldPos,
    /// Blocks fallen into the surface — the burst intensity.
    pub fall: f32,
}

/// Exposure damage due on one mob's own clocks. Stable ids survive earlier deaths.
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

/// Work retained for one mob between the manager's population phases.
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

/// Every species' [`MobMeta`], derived once per content registry from the precached
/// [`Model`](petramond_world::bbmodel::Model)s (see [`model`](super::model)) and indexed by `Mob as
/// usize`. It's identical for every world on one registry, so computing it once keeps each
/// `World::new` (of which the tests make dozens) from re-deriving it — and nothing here
/// re-reads a `.bbmodel`.
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
    /// Advance every mob by one game tick (passing each its species' idle-animation
    /// metadata + ragdoll skeleton) and refresh its cached skylight, then resolve soft
    /// entity pushing and remove any mob that should leave the live world: a finished
    /// death corpse, or a hostile mob that has distance-despawned (culled, and so not
    /// saved). Returns gameplay events the mobs produced this tick: melee strikes for
    /// the player damage pipeline, and landed falls for the mob damage pipeline.
    ///
    /// `player_pos` is the player's body centre — the AI's player anchor for head-look
    /// and distance-despawn. `player_body` is the player's *pushable* body, present only
    /// when the player has a physical presence (a survival body, not a noclip spectator):
    /// when present the mobs are shoved off it (player→mob), on the tick. The reverse —
    /// the mobs shoving the *player* — is NOT done here: that moves the player, which is
    /// integrated per-frame for smoothness, so the caller applies it per-frame via
    /// [`push_on_player`](Self::push_on_player).
    ///
    /// When `freeze_unloaded` is set (a save is attached), a mob standing over a
    /// not-yet-loaded chunk is frozen — not simulated, and excluded from pushing — until
    /// the unload harvests it into that chunk's record. This mirrors the dropped-item
    /// freeze and stops a mob from falling through missing terrain at the streamed edge.
    ///
    /// The [simulation distance](super::SimDistance) decides how much of each living mob's
    /// tick runs: the full tick near a player, physics on a reduced-rate brain
    /// farther out, and nothing but the despawn rule beyond (see `lod`).
    pub fn tick(
        &mut self,
        dt: f32,
        world: &ServerWorld,
        anchors: &[PlayerAnchor],
        freeze_unloaded: bool,
    ) -> MobTickEvents {
        // The start-of-tick AI view, spatially indexed once for every
        // neighbour query and id lookup this tick (see `mob::spatial`).
        let mut ai_mobs = std::mem::take(&mut self.ai_snapshot);
        ai_mobs.rebuild(self.list.iter().map(|m| AiMob {
            id: m.id(),
            kind: m.kind,
            pos: m.pos,
            active: !m.is_dead() && (!freeze_unloaded || terrain_under_mob_is_final(world, m)),
            tags: m.tags_shared(),
        }));
        // Solid-collision bodies as of the start of this tick. Soft mobs use
        // this immutable obstacle view; solid peers propose independently and
        // meet in the relative-motion solver below.
        let solid = self.solid_obstacles();
        // The noise batch every mob hears this tick: everything pushed since the
        // last mob tick, snapshotted BEFORE any mob moves so hearing doesn't
        // depend on iteration order. Mob footsteps recorded below land in
        // `pending_noises` for the next tick.
        self.heard.take_batch(&mut self.pending_noises);
        let mut pending_noises = std::mem::take(&mut self.pending_noises);
        let mut turns = std::mem::take(&mut self.turns);
        turns.clear();
        turns.resize_with(self.list.len(), Turn::default);
        // Moved out so instances can look up / fill shared confined regions
        // while `self.list` is mutably borrowed. The clock feed also expires
        // regions past their maximum age (see `confined::REGION_MAX_AGE_TICKS`).
        self.confined_regions.set_now(world.current_tick());
        let mut confined_regions = std::mem::take(&mut self.confined_regions);

        // What each mob runs this tick, by its distance from the players
        // (see `lod`).
        let now = world.current_tick();
        let mut steps = std::mem::take(&mut self.step_scratch);
        steps.clear();
        steps.extend(
            self.list
                .iter()
                .map(|m| self.sim_distance.step(m, anchors, now)),
        );
        // Route searches share one budget per tick; searches suspended on an
        // earlier tick (and continuing this one) get a reserved share so they
        // always progress.
        self.path_budget.refill(
            self.list
                .iter()
                .zip(&steps)
                .filter(|(m, step)| **step == lod::SimStep::Think && m.nav_search_waiting())
                .count(),
        );

        // Begin every body before any brain reads this tick's population.
        for (i, mob) in self.list.iter_mut().enumerate() {
            if freeze_unloaded && !terrain_under_mob_is_final(world, mob) {
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

        // Shared confinement fills and route budgets retain storage order.
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
                super::append_body_supports(mob.pos, mob.yaw, d.size, &solid, mob.id(), &mut supports);
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
            let ctx = MobTickCtx { dt, inputs: &inputs, anchor: &anchor, def: d, meta };
            turns[i].footing = Some(mob.perceive(&ctx, &mut confined_regions, steps[i] == lod::SimStep::Think));
        }

        // Gather in storage order, dispatch each scripted key once, then feed
        // each reply back to the mob and node that made the request.
        let mut requests = std::mem::take(&mut self.scripted_requests);
        requests.clear();
        for (i, mob) in self.list.iter_mut().enumerate() {
            let Some(footing) = turns[i].footing else { continue };
            if steps[i] != lod::SimStep::Think { continue; }
            let d = def(mob.kind);
            let meta = &MOB_META.current()[mob.kind.0 as usize];
            let anchor = *nearest_anchor(anchors, mob.pos);
            let inputs = TickInputs {
                world, players: anchors, noises: &self.heard, mobs: &ai_mobs,
                path_budget: Some(&self.path_budget), solid: &solid, solid_escape: &solid,
            };
            let ctx = MobTickCtx { dt, inputs: &inputs, anchor: &anchor, def: d, meta };
            let start = requests.len();
            mob.scripted_requests(&ctx, footing, &mut requests);
            turns[i].requests = start..requests.len();
        }
        let mut replies = crate::modding::ai::dispatch_batch(now, &requests);
        requests.clear();
        self.scripted_requests = requests;

        for (i, mob) in self.list.iter_mut().enumerate() {
            let Some(footing) = turns[i].footing else { continue };
            let d = def(mob.kind);
            let meta = &MOB_META.current()[mob.kind.0 as usize];
            let anchor = *nearest_anchor(anchors, mob.pos);
            let peer_obstacles = if d.collision == super::MobCollision::Solid {
                supports.clear();
                super::append_body_supports(mob.pos, mob.yaw, d.size, &solid, mob.id(), &mut supports);
                supports.as_slice()
            } else {
                solid.as_slice()
            };
            let inputs = TickInputs {
                world, players: anchors, noises: &self.heard, mobs: &ai_mobs,
                path_budget: Some(&self.path_budget), solid: peer_obstacles, solid_escape: &solid,
            };
            let ctx = MobTickCtx { dt, inputs: &inputs, anchor: &anchor, def: d, meta };
            let think = steps[i] == lod::SimStep::Think;
            let range = turns[i].requests.clone();
            let decision = mob.think(&ctx, footing, think, ScriptedReplies::new(&mut replies[range]));
            turns[i].start = mob.act(&ctx, footing, &decision, think);
            mob.apply_expression(dt, d, &meta.named_anims, &decision.into());
        }
        self.solid.supports = supports;
        self.confined_regions = confined_regions;
        self.step_scratch = steps;
        self.turns = turns;
        self.solve_solids(world, solid);
        let mut turns = std::mem::take(&mut self.turns);

        // Post-motion bookkeeping observes committed poses, never an
        // overlapping proposal that the pair solver subsequently shortened.
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
            // A walking mob of a noisy species is audible: record its
            // footstep for next tick's batch. Silent bodies (`"step_noise":
            // false` — a boat, a cart) never enter the batch.
            if mob.moving && def(mob.kind).step_noise {
                pending_noises.push(Noise {
                    pos: mob.pos,
                    kind: NoiseKind::Step,
                    source: EntityRef::Mob(mob.id()),
                });
            }
            if let Some(intent) = mob.take_attack() {
                // The knockback direction is derived here, from the live
                // attacker→target geometry at strike time — horizontal, away
                // from the attacker. A target that vanished mid-tick (player
                // disconnected, mob culled) fizzles the strike whole.
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

/// Whether the terrain `mob` stands on has ARRIVED — the freeze gate shared
/// by the tick loop and the push pass, so a mob over not-yet-generated
/// terrain is skipped by both. Its feet cell and the cell under it must read
/// final (a loaded section, or an absent one whose generated summary proves
/// it uniform), and it must be above the world floor. A loaded COLUMN is not
/// enough: the world is cubic, so a deep section can be out of the vertical
/// window while the sections above it are loaded, and a body simulated
/// against that absent floor reads air and falls out of the world (the same
/// rule as `world::entities::terrain_under_drop_is_final`).
fn terrain_under_mob_is_final(world: &ServerWorld, mob: &Instance) -> bool {
    let c = mob.pos.block();
    c.y >= petramond_world::chunk::WORLD_MIN_Y
        && world.physics_cell_final_at(c.x, c.y, c.z)
        && world.physics_cell_final_at(c.x, c.y - 1, c.z)
}
