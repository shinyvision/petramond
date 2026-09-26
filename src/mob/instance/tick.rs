//! One mob's game tick, as the phases the manager runs across the whole
//! population (see `manager::simulation`):
//!
//! 1. [`begin`](Instance::begin) — world-free bookkeeping: the render-pose
//!    snapshot, feedback clocks, shear regrowth, the distance-despawn rule,
//!    and a mod's kinematic placement.
//! 2. [`perceive`](Instance::perceive) — the navigation cell and the confined
//!    verdict, whose region fills are shared through the manager's cache (so
//!    it runs serially, in storage order).
//! 3. [`scripted_requests`](Instance::scripted_requests) — what this mob's
//!    claimed scripted nodes ask their mods; the manager dispatches every
//!    mob's requests in one batch per node key.
//! 4. [`think`](Instance::think) — the brain settles the decision.
//! 5. [`act`](Instance::act) — navigation, steering and body integration.
//! 6. `apply_expression` (see `mob::anim`) — the expression clock.
//!
//! Phases 2–5 spend the tick's shared search budgets in storage order, which
//! is what keeps the tick deterministic.

use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

use super::parts::{ConfinementProbe, HeldDecision, Mind};
use super::Instance;
use crate::mob::brain::{AiCtx, BehaviorOutput, Brain, ScriptedReplies, TickInputs};
use crate::mob::confined::{self, ConfinedRegion, RegionCache};
use crate::mob::kinematics::{route_steering_supported, Locomotion, Surroundings};
use crate::mob::model_meta::{IdleAnimMeta, NamedAnimMeta, Skeleton};
use crate::mob::{nav, path, EntityRef, MobDef, MobRng, MobTagValue, PlayerAnchor};
use crate::modding::ai::AiNodeRequest;

/// A mob with a despawn radius that is farther than this from the player is also
/// eligible for *random* despawn each tick — the churn that recycles far unseen
/// hostiles (deep cave spawns) so the population cap keeps freeing room for new
/// spawns near the player. Inside this distance only the hard radius applies.
const RANDOM_DESPAWN_MIN_DIST: f32 = crate::mob::PLAYER_REACTIVE_RANGE;
/// Per-tick random-despawn chance once eligible: ~40 s expected lifetime at 20 TPS.
const RANDOM_DESPAWN_CHANCE: f32 = 1.0 / 800.0;

/// How far below an airborne arc's peak a route cell must lie for the body to
/// brake over it: a step down, never the landing of a hop along the flat.
const DROP_BRAKE_DEPTH: f64 = 0.9;

/// A species' model-derived data the tick reads, derived once per process.
#[derive(Default)]
pub(in crate::mob) struct SpeciesMeta {
    /// This species' `idle_*` animations (name-sorted; length + loop mode).
    pub idle_anims: Vec<IdleAnimMeta>,
    /// EVERY named animation (name-sorted; length + loop mode) — how a
    /// mod-activated one-shot layer knows when it has played through and
    /// retires itself.
    pub named_anims: Vec<NamedAnimMeta>,
    /// Bone hierarchy (pivots + parents) for the death ragdoll, matching the
    /// renderer's bone order so a sim-computed pose drops into the render bake.
    pub skeleton: Skeleton,
}

/// What a living mob's world-reading phases read besides the mob itself.
pub(in crate::mob) struct MobTickCtx<'a> {
    pub dt: f32,
    /// The tick-wide shared perception state.
    pub inputs: &'a TickInputs<'a>,
    /// The player nearest this mob — the default target of player-anchored
    /// decisions.
    pub anchor: &'a PlayerAnchor,
    pub def: &'static MobDef,
    pub meta: &'a SpeciesMeta,
}

/// Where a living mob stands this tick, as [`perceive`](Instance::perceive)
/// resolved it.
#[derive(Copy, Clone, Debug)]
pub(in crate::mob) struct Footing {
    /// The cell navigation starts from: the standing foothold on land, or the
    /// fluid-surface cell while in fluid.
    pub cell: IVec3,
    /// The fluid the body is in or resting on — navigation footing, not
    /// physical immersion: it stays set while the mob bobs at the surface,
    /// so AI, repathing and steering treat a swimmer as supported.
    pub fluid: Option<Block>,
}

/// What [`begin`](Instance::begin) left the rest of a mob's tick to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::mob) enum Begun {
    /// A corpse: only its ragdoll advances.
    Corpse,
    /// A mod placed the body this tick: only its expression runs.
    Placed,
    /// A living, simulating mob.
    Live,
}

/// The motion proposal [`act`](Instance::act) hands the manager's solid-peer
/// solve: the pre-integration ground state and where the peer motion starts.
pub(in crate::mob) type MotionStart = Option<(bool, WorldPos)>;

/// The deciding mob's own half of its [`AiCtx`], borrowed from disjoint parts
/// of the instance so the brain can be borrowed mutably beside it.
struct Myself<'a> {
    id: u64,
    pos: WorldPos,
    yaw: f32,
    contacts: &'a [EntityRef],
    target: Option<EntityRef>,
    attacker: Option<(EntityRef, u32)>,
    nav_idle: bool,
    tags: &'a std::sync::Arc<std::collections::BTreeMap<String, MobTagValue>>,
    region: Option<&'a ConfinedRegion>,
}

/// The AI context for `me` this tick.
fn ai_ctx<'a>(
    ctx: &'a MobTickCtx<'a>,
    footing: Footing,
    me: Myself<'a>,
    scripted: ScriptedReplies<'a>,
    rng: &'a mut MobRng,
) -> AiCtx<'a> {
    let d = ctx.def;
    let inputs = ctx.inputs;
    AiCtx {
        reach: Some(inputs.world.reach_budget()),
        mob_id: me.id,
        pos: me.pos,
        cell: footing.cell,
        yaw: me.yaw,
        head_height: d.size.height,
        half_width: d.size.half_width,
        world: inputs.world,
        player_id: ctx.anchor.id,
        player_pos: ctx.anchor.pos,
        player_sneaking: ctx.anchor.sneaking,
        player_held: ctx.anchor.held,
        players: inputs.players,
        noises: inputs.noises,
        contacts: me.contacts,
        target: me.target,
        attacker: me.attacker,
        nav_idle: me.nav_idle,
        in_fluid: footing.fluid,
        head: d.size.head_cells(),
        tolerated: d.tolerates.blocks,
        idle_anims: &ctx.meta.idle_anims,
        mob_index: inputs.mobs.by_id(me.id).map(|(i, _)| i),
        mobs: inputs.mobs,
        tags: me.tags,
        confined_region: me.region,
        scripted,
        rng,
    }
}

impl Instance {
    /// Phase 1 — the world-free start of a simulated tick: snapshot the
    /// render pose, count the feedback clocks down, age the attacker memory,
    /// regrow a shorn coat, apply the distance-despawn rule against the
    /// nearest player at `anchor_pos`, and write a mod's kinematic pose.
    pub(in crate::mob) fn begin(
        &mut self,
        dt: f32,
        anchor_pos: WorldPos,
        despawn_radius: Option<f32>,
    ) -> Begun {
        self.snapshot_interp();
        // The attack latch is strictly this-tick state: clear before any early
        // return so a mob that died mid-swing can't land a stale strike.
        self.combat.attack = None;
        // Dead: freeze the body (pos/yaw stay put — they're the ragdoll's
        // `global`); only the ragdoll advances, and the kill's red flash
        // fades out over these first ticks.
        if self.combat.death.is_dead() {
            self.clear_drive();
            self.combat.count_down_feedback(dt);
            return Begun::Corpse;
        }
        self.combat.count_down_feedback(dt);
        self.combat.age_attacker();
        self.regrow_coat();
        self.check_distance_despawn(anchor_pos, despawn_radius);
        // A mod-authored pose replaces the whole locomotion step: no
        // navigation, no brain locomotion, no integration — the body is
        // written where the mod put it. Expression (named animation layers)
        // still advances, so a driven vehicle's wheels keep turning. There is
        // no motion proposal: the solid-peer solve sees a stationary body at
        // its new pose, and fall bookkeeping re-anchors inside the placement.
        if let Some(pose) = self.motion.kinematic.take() {
            self.motion.drive = None;
            self.place_kinematic(dt, pose);
            return Begun::Placed;
        }
        self.level_body(dt);
        Begun::Live
    }

    /// Snapshot the pose this tick starts from, for render interpolation.
    fn snapshot_interp(&mut self) {
        self.interp = super::parts::Interp {
            pos: self.pos,
            yaw: self.yaw,
            tilt: self.tilt,
            anim_time: self.anim_time,
            head_yaw: self.head_yaw,
            head_pitch: self.head_pitch,
            hurt: self.combat.hurt_timer,
        };
    }

    /// Shear regrowth counts down on the tick; at zero the tag is removed and
    /// the coat is back. Pauses with the rest of the sim while the mob's chunk
    /// is unloaded (this tick is simply not run), like the dropped-item
    /// timers. Gated on presence so a coated mob never touches (and never
    /// CoW-clones) its snapshot-shared tag map here.
    fn regrow_coat(&mut self) {
        let regrow = self.tag_int(crate::mob::tags::SHEAR_REGROW);
        if regrow == 1 {
            self.tags_mut().remove(crate::mob::tags::SHEAR_REGROW);
        } else if regrow > 1 {
            self.tags_mut().insert(
                crate::mob::tags::SHEAR_REGROW.to_owned(),
                MobTagValue::Int(regrow - 1),
            );
        }
    }

    /// Phase 2 — perception: resolve the cell navigation starts from and
    /// refresh the confined verdict (the `petramond:confined` tag, written
    /// ONLY on a transition: the map is copy-on-write shared with the AI
    /// snapshot, so a redundant write would still deep-clone it). A block
    /// change near this mob's pen drops its cached region, which forces the
    /// re-check off-cadence instead of waiting out the interval. A coasting
    /// mob (`think` false) keeps its verdict.
    pub(in crate::mob) fn perceive(
        &mut self,
        ctx: &MobTickCtx,
        regions: &mut RegionCache,
        think: bool,
    ) -> Footing {
        let world = ctx.inputs.world;
        let d = ctx.def;
        // Navigation's cell probes share one collision-derived classification:
        // `solid` = cells whose boxes fill the whole cell, `support` = cells
        // with any collision at all (a slab, a bed, a ladder column can bear
        // feet without blanket-blocking their cell). See `mob::nav`. ONE
        // cursor serves the navigation-cell resolve and the confinement fill.
        let cursor = world.cursor();
        let solid = nav::nav_solid_fn(&cursor);
        let support = nav::nav_support_fn(&cursor, d.size.half_width);
        let fluid = nav::nav_fluid_fn(&cursor);
        let footing_fluid = nav::fluid_footing(&cursor, self.pos);
        let on_fluid = footing_fluid.is_some();
        // The standing foothold on land (robust to standing at a block edge,
        // where the cell under the centre overhangs into air), or the
        // fluid-surface cell while in fluid. `navigation_cell_with` explains
        // why a mob in fluid must never path from its submerged standing cell.
        let cell = path::navigation_cell_with(
            self.pos,
            d.size.half_width,
            d.size.head_cells(),
            on_fluid,
            &solid,
            &support,
            &fluid,
        )
        .unwrap_or_else(|| self.pos.block());
        let probe = ConfinementProbe {
            cell,
            think,
            judgeable: self.motion.on_ground && !on_fluid,
            nav_rev: world.nav_revision(),
        };
        let verdict = self.confinement.refresh(probe, regions, || {
            let params = d.path_params();
            let step_allowed = nav::navigation_step_gate(&cursor, params, d.size.height);
            let loaded = nav::nav_loaded_fn(&cursor);
            confined::confined_region(
                cell,
                params,
                &solid,
                &support,
                &fluid,
                &step_allowed,
                &loaded,
            )
        });
        if let Some(confined) = verdict {
            self.set_confined(confined);
        }
        Footing {
            cell,
            fluid: footing_fluid,
        }
    }

    /// Write the confined verdict as the `petramond:confined` tag, on a
    /// transition only.
    fn set_confined(&mut self, confined: bool) {
        if confined == self.is_confined() {
            return;
        }
        if confined {
            self.tags_mut().insert(
                crate::mob::tags::CONFINED.to_owned(),
                MobTagValue::Bool(true),
            );
        } else {
            self.tags_mut().remove(crate::mob::tags::CONFINED);
        }
    }

    /// Phase 3 — the requests of this mob's CLAIMED scripted nodes, in the
    /// order its brain visits them, appended to `out`. The replies come back
    /// through [`think`](Self::think)'s `scripted`, one per request.
    pub(in crate::mob) fn scripted_requests(
        &mut self,
        ctx: &MobTickCtx,
        footing: Footing,
        out: &mut Vec<AiNodeRequest>,
    ) {
        if !self.mind.brain.has_scripted() {
            return;
        }
        let (me, brain, _, rng) = self.myself();
        let ai = ai_ctx(ctx, footing, me, ScriptedReplies::default(), rng);
        out.extend(brain.scripted_nodes().filter_map(|node| node.request(&ai)));
    }

    /// Phase 4 — decide: the brain settles this tick's decision (a coasting
    /// mob replays the continuous channels of its last one), and the latched
    /// parts land — the strike, the target, the speed, the facing, and the
    /// scripted nodes' tag writes. `scripted` carries this mob's replies from
    /// the batched dispatch.
    pub(in crate::mob) fn think(
        &mut self,
        ctx: &MobTickCtx,
        footing: Footing,
        think: bool,
        scripted: ScriptedReplies,
    ) -> BehaviorOutput {
        let decision = if think {
            let (me, brain, held, rng) = self.myself();
            let mut ai = ai_ctx(ctx, footing, me, scripted, rng);
            let decision = brain.decide(&mut ai);
            *held = HeldDecision::of(&decision);
            decision
        } else {
            self.mind.held_decision.replay(self.mind.current_target)
        };
        self.combat.attack = decision.attack;
        self.mind.current_target = decision.target;
        self.motion.walk_speed_scale = decision
            .speed_scale
            .filter(|scale| scale.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, mod_api::MAX_MOB_SPEED_SCALE);
        if self.combat.stagger_timer <= 0.0 {
            if let Some(facing) = decision.facing.filter(|angle| angle.is_finite()) {
                self.yaw = crate::mob::kinematics::turn_toward(
                    self.yaw,
                    facing,
                    ctx.def.turn_rate * ctx.dt,
                );
            }
        }
        self.apply_tag_writes(&decision.tag_writes);
        decision
    }

    /// The deciding-mob half of the AI context, the brain, the held
    /// decision and the RNG, borrowed disjointly.
    fn myself(&mut self) -> (Myself<'_>, &mut Brain, &mut HeldDecision, &mut MobRng) {
        let Instance {
            id,
            pos,
            yaw,
            mind,
            combat,
            tags,
            confinement,
            rng,
            ..
        } = self;
        let Mind {
            brain,
            nav,
            current_target,
            held_decision,
            contacts,
            ..
        } = mind;
        let me = Myself {
            id: *id,
            pos: *pos,
            yaw: *yaw,
            contacts,
            target: *current_target,
            attacker: combat.attacker.map(|who| (who, combat.attacker_ticks)),
            nav_idle: nav.is_idle(),
            tags,
            region: confinement.region(),
        };
        (me, brain, held_decision, rng)
    }

    /// Tag writes carried back by scripted decisions land HERE, after the
    /// whole brain decided — the engine-applied half of the detached dispatch
    /// contract (`AiNodeDecision::tags`). Same cap rule as the `MobTagSet`
    /// HostCall: a NEW key past the cap is refused.
    fn apply_tag_writes(&mut self, writes: &[(String, Option<MobTagValue>)]) {
        for (key, value) in writes {
            match value {
                Some(v) => {
                    if self.tags.len() >= crate::mob::MAX_MOB_TAGS && !self.tags.contains_key(key)
                    {
                        log::warn!(
                            "mob {} tag map full; decision write '{key}' dropped",
                            self.id
                        );
                        continue;
                    }
                    self.tags_mut().insert(key.clone(), v.clone());
                }
                None => {
                    if self.tags.contains_key(key) {
                        self.tags_mut().remove(key);
                    }
                }
            }
        }
    }

    /// Phase 5 — act: hand the decision's goal to the navigator (on a
    /// thinking tick), steer along the route, veer around touching bodies,
    /// avoid hazards, and integrate the body. Returns the motion proposal for
    /// the manager's solid-peer solve.
    pub(in crate::mob) fn act(
        &mut self,
        ctx: &MobTickCtx,
        footing: Footing,
        decision: &BehaviorOutput,
        think: bool,
    ) -> MotionStart {
        let dt = ctx.dt;
        let d = ctx.def;
        let inputs = ctx.inputs;
        let world = inputs.world;
        let on_fluid = footing.fluid.is_some();
        let cursor = world.cursor();
        // The model-aware box source for body collision (legs/top of a bbmodel block).
        let boxes = |x: i32, y: i32, z: i32| cursor.collision_boxes_xyz(x, y, z);
        let can_repath = self.motion.on_ground || on_fluid;
        let can_steer = d.air_control
            || route_steering_supported(self.motion.on_ground, on_fluid, self.motion.vel.y);
        // The pathfinder treats every OTHER entity as a soft obstacle to bend
        // around — except the brain's current target (a zombie paths TO the
        // player it hunts, never around them).
        let nav_inputs = nav::NavInputs {
            self_id: self.id,
            target: self.mind.current_target,
            mobs: inputs.mobs,
            players: inputs.players,
            budget: inputs.path_budget,
        };
        if think {
            self.mind.nav.update_goal_when_supported(
                decision.goal,
                footing.cell,
                world,
                can_repath,
                &nav_inputs,
            );
        }
        let (wish, jump) = if can_steer {
            self.mind
                .nav
                .follow_steered(self.pos, self.motion.on_ground, world)
        } else {
            self.drift_along_route();
            (Vec3::ZERO, false)
        };
        // A wish driving straight into a touching body veers around it, so two
        // mobs pressing each other slide past instead of cancelling out. A
        // jump approach is left alone (the ledge face ahead IS the route).
        let wish = if jump {
            wish
        } else {
            self.mind.unstick.steer(
                wish,
                self.pos,
                self.id,
                d.size.half_width,
                &self.mind.contacts,
                self.mind.current_target,
                inputs.mobs,
                inputs.players,
            )
        };
        let immersion = world
            .data()
            .body_fluid(self.pos, d.size.height, d.buoyancy);
        // A mod's horizontal drive is that mod's own steering (a player-driven
        // vehicle crosses whatever its driver steers it over).
        let driven = self
            .motion
            .drive
            .is_some_and(|drive| drive.horizontal.is_some());
        let (wish, jump) = if driven {
            (wish, jump)
        } else {
            self.mind.nav.avoid_hazards(
                self.pos,
                self.yaw,
                d.size,
                wish,
                jump,
                d.walk_speed * self.motion.walk_speed_scale * dt,
                &cursor,
            )
        };
        let current = world
            .data()
            .body_current(self.pos, d.size.height, immersion);
        let was_on_ground = self.motion.on_ground;
        let motion_start = self.pos;
        let healed = self.integrate_locomotion(
            dt,
            d,
            Locomotion {
                wish,
                jump,
                can_steer,
            },
            &Surroundings {
                boxes: &boxes,
                obstacles: inputs.solid,
                escape_obstacles: inputs.solid_escape,
                immersion,
                current,
            },
        );
        // Post-move arrival: feed the tick's LANDED position back to the
        // route before anything reads this tick's state. A landing inside
        // the goal's arrive window must end the walk THIS tick — the route
        // otherwise notices only next tick, and the one-tick drive-intent
        // pipeline (a mod gait policy latching on `moving`) fires one stale
        // parting hop AT the destination. Also stops the walk clip exactly
        // at arrival instead of one step past it.
        if self.motion.on_ground {
            self.mind.nav.advance_cursor(self.pos);
            // A driven step walks with no route at all: it is not a walk
            // that has arrived.
            if self.mind.nav.is_idle() && !self.motion.stepping {
                self.moving = false;
                self.motion.air_walk = false;
            }
        }
        Some((was_on_ground, motion_start + Vec3::from(healed)))
    }

    /// Route bookkeeping while steering is suspended (falling): a ballistic
    /// arc (a mod-driven hop's descent, a knockback flight) passes waypoints
    /// between steered ticks, and the cursor must advance past them or
    /// steering resumes pointed backward.
    fn drift_along_route(&mut self) {
        self.mind.nav.advance_cursor(self.pos);
        // An arc stops reading as a WALK the moment its route completes
        // (arrival consumed mid-descent): the walk expression — and any mod
        // gait policy gating launches on `moving` — must not outlive the
        // navigation that drove it, or the one-tick drive-intent pipeline
        // fires one stale parting hop AT the destination.
        if self.mind.nav.is_idle() {
            self.motion.air_walk = false;
        }
        // Dropping to a lower cell of its route, the body stops drifting
        // once it is over that cell. A level arc (a hop along flat ground)
        // keeps its carry.
        if self.motion.vel.y < 0.0 {
            if let Some(landing) = self.mind.nav.landing_under(self.pos, self.motion.vel) {
                if f64::from(landing.y) + DROP_BRAKE_DEPTH <= self.motion.fall_peak_y {
                    self.motion.vel.x = 0.0;
                    self.motion.vel.z = 0.0;
                }
            }
        }
    }

    /// Distance-despawn: a mob with a row-level radius is culled immediately once it
    /// is outside that radius, and randomly once beyond the eligibility distance.
    /// Species with no radius persist while loaded.
    fn check_distance_despawn(&mut self, player_pos: WorldPos, despawn_radius: Option<f32>) {
        if let Some(radius) = despawn_radius {
            let dist2 = (self.pos - player_pos).length_squared();
            // The roll is drawn only when eligible, so a near mob's brain RNG
            // stream is untouched by this rule.
            self.distance_despawned = despawn_now(dist2, radius, || self.rng.next_f32());
        } else {
            self.distance_despawned = false;
        }
    }

    /// A tick skipped for simulation distance (see `manager::lod`): nothing
    /// simulates, but the distance-despawn rule still applies, and the pose
    /// is held so the body rests exactly where it stopped instead of
    /// replaying its last interpolation step.
    pub(in crate::mob) fn tick_frozen(&mut self, player_pos: WorldPos, despawn_radius: Option<f32>) {
        self.snapshot_interp();
        self.check_distance_despawn(player_pos, despawn_radius);
    }

    /// Every phase of one tick for this mob alone, in order — the fixtures'
    /// single-body driver (the manager interleaves the phases across the
    /// population instead).
    #[cfg(test)]
    pub(in crate::mob) fn tick_alone(
        &mut self,
        ctx: &MobTickCtx,
        regions: &mut RegionCache,
        think: bool,
    ) -> MotionStart {
        let expression = crate::mob::anim::Expression::default();
        match self.begin(ctx.dt, ctx.anchor.pos, ctx.def.despawn_radius) {
            Begun::Corpse => {
                self.tick_ragdoll(ctx.dt, ctx.inputs.world, ctx.def, &ctx.meta.skeleton);
                None
            }
            Begun::Placed => {
                self.apply_expression(ctx.dt, ctx.def, &ctx.meta.named_anims, &expression);
                None
            }
            Begun::Live => {
                let footing = self.perceive(ctx, regions, think);
                let mut requests = Vec::new();
                if think {
                    self.scripted_requests(ctx, footing, &mut requests);
                }
                let mut replies = crate::modding::ai::dispatch_batch(
                    ctx.inputs.world.current_tick(),
                    &requests,
                );
                let decision = self.think(ctx, footing, think, ScriptedReplies::new(&mut replies));
                let start = self.act(ctx, footing, &decision, think);
                self.apply_expression(ctx.dt, ctx.def, &ctx.meta.named_anims, &decision.into());
                start
            }
        }
    }
}

/// The per-tick despawn decision for a mob with despawn radius `radius` at squared
/// player distance `dist2`: certain at/beyond the hard radius, a small random chance
/// (`roll`, drawn lazily in `[0, 1)`) once beyond the eligibility distance, never
/// closer than that. Factored out pure so the eligibility rules are tested without
/// simulating a mob.
fn despawn_now(dist2: f32, radius: f32, roll: impl FnOnce() -> f32) -> bool {
    if dist2 >= radius * radius {
        return true;
    }
    dist2 >= RANDOM_DESPAWN_MIN_DIST * RANDOM_DESPAWN_MIN_DIST && roll() < RANDOM_DESPAWN_CHANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn despawn_is_certain_at_radius_random_when_far_never_when_near() {
        let r = 128.0;
        // At/beyond the hard radius: certain, no roll consumed.
        assert!(despawn_now(r * r, r, || unreachable!(
            "no roll at the hard radius"
        )));
        // Beyond the eligibility distance but inside the radius: decided by the roll.
        let far2 = (RANDOM_DESPAWN_MIN_DIST + 1.0).powi(2);
        assert!(despawn_now(far2, r, || 0.0));
        assert!(!despawn_now(far2, r, || RANDOM_DESPAWN_CHANCE));
        // Near the player: never, and the RNG stream is untouched.
        let near2 = (RANDOM_DESPAWN_MIN_DIST - 1.0).powi(2);
        assert!(!despawn_now(near2, r, || unreachable!(
            "no roll near the player"
        )));
    }
}
