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

const RANDOM_DESPAWN_MIN_DIST: f32 = crate::mob::PLAYER_REACTIVE_RANGE;
const RANDOM_DESPAWN_CHANCE: f32 = 1.0 / 800.0;

const DROP_BRAKE_DEPTH: f64 = 0.9;

#[derive(Default)]
pub(in crate::mob) struct SpeciesMeta {
    pub idle_anims: Vec<IdleAnimMeta>,
    pub named_anims: Vec<NamedAnimMeta>,
    pub skeleton: Skeleton,
}

pub(in crate::mob) struct MobTickCtx<'a> {
    pub dt: f32,
    pub inputs: &'a TickInputs<'a>,
    pub anchor: &'a PlayerAnchor,
    pub def: &'static MobDef,
    pub meta: &'a SpeciesMeta,
}

#[derive(Copy, Clone, Debug)]
pub(in crate::mob) struct Footing {
    pub cell: IVec3,
    pub fluid: Option<Block>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::mob) enum Begun {
    Corpse,
    Placed,
    Live,
}

pub(in crate::mob) type MotionStart = Option<(bool, WorldPos)>;

struct Myself<'a> {
    id: u64,
    pos: WorldPos,
    yaw: f32,
    contacts: &'a [EntityRef],
    target: Option<EntityRef>,
    attacker: Option<(EntityRef, u32)>,
    nav_idle: bool,
    tags: &'a std::sync::Arc<std::collections::BTreeMap<String, MobTagValue>>,
    tags_rev: u64,
    region: Option<&'a ConfinedRegion>,
}

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
        tags_rev: me.tags_rev,
        confined_region: me.region,
        scripted,
        rng,
    }
}

impl Instance {
    pub(in crate::mob) fn begin(
        &mut self,
        dt: f32,
        anchor_pos: WorldPos,
        despawn_radius: Option<f32>,
    ) -> Begun {
        self.snapshot_interp();
        self.combat.attack = None;
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

    /// Phase 2, perception: pick the cell nav starts from and refresh the confined verdict.
    /// The `petramond:confined` tag is only written on a transition, because the map is
    /// copy-on-write shared with the AI snapshot and even a redundant write deep-clones it. A
    /// block change near the pen drops the cached region and forces an off-cadence recheck. A
    /// coasting mob (`think` false) keeps its old verdict.
    pub(in crate::mob) fn perceive(
        &mut self,
        ctx: &MobTickCtx,
        regions: &mut RegionCache,
        think: bool,
    ) -> Footing {
        let world = ctx.inputs.world;
        let d = ctx.def;
        let cursor = world.cursor();
        let solid = nav::nav_solid_fn(&cursor);
        let support = nav::nav_support_fn(&cursor, d.size.half_width);
        let fluid = nav::nav_fluid_fn(&cursor);
        let footing_fluid = nav::fluid_footing(&cursor, self.pos);
        let on_fluid = footing_fluid.is_some();
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
            terrain_rev: world.terrain_revision(),
        };
        let verdict = self.confinement.refresh(probe, regions, || {
            let params = d.path_params();
            let step_allowed = nav::navigation_step_gate(&cursor, params, d.size.height);
            let loaded = nav::nav_loaded_fn(&cursor);
            confined::confinement_verdict(
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

    fn myself(&mut self) -> (Myself<'_>, &mut Brain, &mut HeldDecision, &mut MobRng) {
        let Instance {
            id,
            pos,
            yaw,
            mind,
            combat,
            tags,
            tags_rev,
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
            tags_rev: *tags_rev,
            region: confinement.region(),
        };
        (me, brain, held_decision, rng)
    }

    fn apply_tag_writes(&mut self, writes: &[(String, Option<MobTagValue>)]) {
        for (key, value) in writes {
            match value {
                Some(v) => {
                    if self.tags.len() >= crate::mob::MAX_MOB_TAGS && !self.tags.contains_key(key) {
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
        let boxes = |x: i32, y: i32, z: i32| cursor.collision_boxes_xyz(x, y, z);
        let can_repath = self.motion.on_ground || on_fluid;
        let can_steer = d.air_control
            || route_steering_supported(self.motion.on_ground, on_fluid, self.motion.vel.y);
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
        let immersion = world.data().body_fluid(self.pos, d.size.height, d.buoyancy);
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
            if self.mind.nav.is_idle() && !self.motion.stepping {
                self.moving = false;
                self.motion.air_walk = false;
            }
        }
        Some((was_on_ground, motion_start + Vec3::from(healed)))
    }

    fn drift_along_route(&mut self) {
        self.mind.nav.advance_cursor(self.pos);
        if self.mind.nav.is_idle() {
            self.motion.air_walk = false;
        }
        if self.motion.vel.y < 0.0 {
            if let Some(landing) = self.mind.nav.landing_under(self.pos, self.motion.vel) {
                if f64::from(landing.y) + DROP_BRAKE_DEPTH <= self.motion.fall_peak_y {
                    self.motion.vel.x = 0.0;
                    self.motion.vel.z = 0.0;
                }
            }
        }
    }

    fn check_distance_despawn(&mut self, player_pos: WorldPos, despawn_radius: Option<f32>) {
        if let Some(radius) = despawn_radius {
            let dist2 = (self.pos - player_pos).length_squared();
            self.distance_despawned = despawn_now(dist2, radius, || self.rng.next_f32());
        } else {
            self.distance_despawned = false;
        }
    }

    pub(in crate::mob) fn tick_frozen(
        &mut self,
        player_pos: WorldPos,
        despawn_radius: Option<f32>,
    ) {
        self.snapshot_interp();
        self.check_distance_despawn(player_pos, despawn_radius);
    }

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
                let mut replies =
                    crate::modding::ai::dispatch_batch(ctx.inputs.world.current_tick(), &requests);
                let decision = self.think(ctx, footing, think, ScriptedReplies::new(&mut replies));
                let start = self.act(ctx, footing, &decision, think);
                self.apply_expression(ctx.dt, ctx.def, &ctx.meta.named_anims, &decision.into());
                start
            }
        }
    }
}

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
        assert!(despawn_now(r * r, r, || unreachable!(
            "no roll at the hard radius"
        )));
        let far2 = (RANDOM_DESPAWN_MIN_DIST + 1.0).powi(2);
        assert!(despawn_now(far2, r, || 0.0));
        assert!(!despawn_now(far2, r, || RANDOM_DESPAWN_CHANCE));
        let near2 = (RANDOM_DESPAWN_MIN_DIST - 1.0).powi(2);
        assert!(!despawn_now(near2, r, || unreachable!(
            "no roll near the player"
        )));
    }
}
