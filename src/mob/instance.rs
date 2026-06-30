//! A live mob instance: shared kinematics + its brain + its navigator.
//!
//! Everything physical a mob does — gravity, axis-resolved block collision, the jump
//! impulse, turning to face travel, advancing the walk cycle — lives here once and
//! is shared by every species; a species differs only by its [`MobDef`] stats and
//! its brain's behaviors. One [`tick`](Instance::tick) is one **game tick** (20 TPS):
//! the brain picks a goal, the navigator turns it into a wish-direction + jump, and
//! the kinematics integrate it. The previous tick's pose is snapshotted each tick so
//! the renderer can interpolate between ticks for smooth motion at any frame rate.

use std::f32::consts::{PI, TAU};

use crate::mathh::{voxel_at, IVec3, Vec3};
use crate::world::World;

use super::brain::{AiCtx, BehaviorOutput, Brain};
use super::model_meta::{IdleAnimMeta, Skeleton};
use super::nav::Navigator;
use super::path;
use super::push;
use super::ragdoll::Ragdoll;
use super::{def, Mob, MobDef, MobRng};

/// Downward acceleration (m/s²) applied to airborne mobs.
const GRAVITY: f32 = -22.0;
/// How long (seconds) the hurt flash + knockback stagger lasts after a non-lethal hit.
/// During it the mob's locomotion is suppressed so the knockback reads, and the
/// renderer tints it red.
const HURT_FLASH_SECS: f32 = 0.3;
/// Horizontal speed (m/s) imparted away from the attacker on a non-lethal hit.
const KNOCKBACK_SPEED: f32 = 6.5;
/// One-shot upward pop (m/s) on a non-lethal hit — a small hop, like a soft jump.
const KNOCKBACK_UP: f32 = 4.2;
/// Per-tick decay of the horizontal knockback velocity during the stagger.
const KNOCKBACK_DAMP: f32 = 0.75;
/// Upward swim speed a mob eases toward whenever its body is under water. A mob has no
/// jump key, so it always swims up — exactly like a player holding jump in water.
/// Mirrors the player's `SWIM_RISE`: the mob rises, breaches the surface (the probe
/// clears the water), gravity then pulls it back, it re-enters and rises again —
/// bobbing through the waterline.
const SWIM_RISE: f32 = 3.0;
/// How fast vertical velocity eases toward the swim target (m/s²) — a soft approach
/// (mirrors the player's `SWIM_VACCEL`) so falling into water decelerates smoothly and
/// the bob rocks instead of snapping.
const SWIM_VACCEL: f32 = 14.0;
/// Fraction of body height at which the "submerged enough to swim" probe sits (≈ the
/// player's thigh-height probe). The mob keeps swimming up until this point clears the
/// water, so its body breaks the surface before gravity takes back over.
const SWIM_PROBE_FRAC: f32 = 1.0 / 3.0;
/// Firm upward boost (m/s) a swimming mob gets when steering toward a 1-block ledge it
/// can climb onto — enough to crest the waterline and land on the block instead of
/// hugging its base forever. Mirrors the player's `SWIM_CLIMB`.
const SWIM_CLIMB: f32 = 4.5;
/// Target horizontal drift speed (m/s) imparted by flowing water — matched to the
/// player's so a mob and the player ride the same current at the same pace. Below walk
/// speed, so a current carries an idle mob but never overpowers a mob that's swimming.
const WATER_CURRENT_SPEED: f32 = 0.75;
/// How fast the head turns toward its look target (rad/s) — deliberately slow so the
/// head pans rather than snaps.
const HEAD_TURN_RATE: f32 = 4.0;

/// How many consecutive game ticks a hostile mob must spend beyond its category's
/// [`despawn_radius`](super::MobCategory::despawn_radius) from the player before it is
/// culled. A short window (1 s at 20 TPS) so a strayed mob vanishes promptly while a
/// momentary excursion doesn't. Tunable; passive mobs (no despawn radius) ignore it.
const HOSTILE_DESPAWN_TICKS: u32 = 20;

/// A live mob. Render-facing fields (`pos`/`yaw`/`anim_time`/`moving`/`skylight` and
/// their `prev_*` snapshots) are public for the scene adapter; the AI/physics state
/// is private.
pub struct Instance {
    pub kind: Mob,
    pub pos: Vec3,
    pub yaw: f32,
    /// Seconds into the currently-playing animation (walk or idle_*; free-running, the
    /// renderer wraps it). Reset to 0 when the active animation changes.
    pub anim_time: f32,
    /// Did the mob have walking locomotion this tick? Selects walk vs idle/rest pose.
    pub moving: bool,
    /// Which `idle_*` animation is playing (index), or `None` for walk / neutral rest.
    pub idle_anim: Option<u8>,
    /// Head orientation **relative to the body** (radians), eased toward the head-look
    /// AI's target. The renderer applies it to the model's `head` bone.
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub skylight: u8,
    /// Previous-tick pose, for render interpolation.
    pub prev_pos: Vec3,
    pub prev_yaw: f32,
    pub prev_anim_time: f32,
    pub prev_head_yaw: f32,
    pub prev_head_pitch: f32,
    /// Hurt-flash intensity last tick, for render interpolation.
    pub prev_hurt: f32,

    vel: Vec3,
    on_ground: bool,
    /// Current health; at `0` the mob dies and `death` becomes `Some`.
    health: f32,
    /// Consecutive game ticks spent beyond the despawn radius from the player. Only a
    /// hostile species advances it (passive mobs have no despawn radius); at
    /// [`HOSTILE_DESPAWN_TICKS`] the manager culls the mob. Reset whenever a player is
    /// near, and never persisted — a reloaded mob re-accumulates from zero.
    despawn_timer: u32,
    /// Seconds of hurt flash + stagger remaining (non-lethal hits). Drives the red
    /// tint and suppresses locomotion so the knockback reads.
    hurt_timer: f32,
    /// Horizontal knockback velocity (m/s), decaying over the stagger. Kept separate
    /// from `vel` so the per-tick wish-velocity overwrite can't wipe it.
    knockback: Vec3,
    /// Soft entity-push velocity (m/s, horizontal) accumulated from overlapping other
    /// entities last tick — added on top of locomotion in [`integrate`](Self::integrate)
    /// and consumed there (the push pass re-derives it each tick from the live overlap).
    /// Kept separate from `vel` for the same reason as `knockback`: the wish-velocity
    /// overwrite would otherwise wipe it.
    push: Vec3,
    /// `Some` once the mob has died — the physics ragdoll that plays before it
    /// despawns. While set, the mob runs no AI and takes no further damage.
    death: Option<Ragdoll>,
    /// The animation kind playing last tick, to detect changes (and reset `anim_time`).
    anim_kind: AnimKind,
    brain: Brain,
    nav: Navigator,
    rng: MobRng,
}

/// Which animation a mob is playing — drives `anim_time` advance rate + reset.
#[derive(Copy, Clone, PartialEq, Eq)]
enum AnimKind {
    Walk,
    Idle(u8),
    Rest,
}

impl Instance {
    /// Spawn a mob of `kind` at `pos` (feet) facing `yaw`. `seed` makes its AI
    /// deterministic and distinct per mob.
    pub fn new(kind: Mob, pos: Vec3, yaw: f32, seed: u64) -> Self {
        let d = def(kind);
        Instance {
            kind,
            pos,
            yaw,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            skylight: 63,
            prev_pos: pos,
            prev_yaw: yaw,
            prev_anim_time: 0.0,
            prev_head_yaw: 0.0,
            prev_head_pitch: 0.0,
            prev_hurt: 0.0,
            vel: Vec3::ZERO,
            on_ground: false,
            health: d.max_health,
            despawn_timer: 0,
            hurt_timer: 0.0,
            knockback: Vec3::ZERO,
            push: Vec3::ZERO,
            death: None,
            anim_kind: AnimKind::Rest,
            brain: (d.make_brain)(d),
            nav: Navigator::new(d.size.head_cells()),
            rng: MobRng::new(seed),
        }
    }

    /// Apply `amount` damage from a point `from` (the attacker). Returns `true` if this
    /// hit was lethal. A dead mob ignores damage (no double-kill, no knockback on a
    /// corpse). Every hit — lethal or not — starts the red hurt flash (so the killing
    /// blow reads the same as any other); a non-lethal hit also knocks the mob back away
    /// from `from`, while a lethal hit instead launches the death ragdoll.
    pub fn hurt(&mut self, amount: f32, from: Vec3) -> bool {
        if self.death.is_some() {
            return false;
        }
        self.health -= amount;
        if self.health <= 0.0 {
            self.health = 0.0;
            // The killing blow flings the corpse in the punched direction (away from the
            // attacker, horizontally); the ragdoll launches + somersaults along it.
            let mut away = self.pos - from;
            away.y = 0.0;
            let launch = away.normalize_or_zero();
            // Ragdoll is initialised on the next tick (which has world access to find
            // the floor). Seed it from this mob's RNG stream for a distinct fling.
            self.death = Some(Ragdoll::pending(self.rng.next_u64(), launch));
            self.knockback = Vec3::ZERO;
            // Flash red on the kill too, so it looks like any other hit (the flash fades
            // over the first moments of the ragdoll).
            self.hurt_timer = HURT_FLASH_SECS;
            self.moving = false;
            self.idle_anim = None;
            return true;
        }
        self.hurt_timer = HURT_FLASH_SECS;
        let mut away = self.pos - from;
        away.y = 0.0;
        self.knockback = away.normalize_or_zero() * KNOCKBACK_SPEED;
        self.vel.y = KNOCKBACK_UP; // one-shot vertical pop, like a jump impulse
        self.on_ground = false;
        false
    }

    /// The mob's collision/selection AABB: feet at `pos`, extending up by the body
    /// height and out by its half-width. The single source of truth for ray-vs-mob.
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let s = def(self.kind).size;
        (
            Vec3::new(
                self.pos.x - s.half_width,
                self.pos.y,
                self.pos.z - s.half_width,
            ),
            Vec3::new(
                self.pos.x + s.half_width,
                self.pos.y + s.height,
                self.pos.z + s.half_width,
            ),
        )
    }

    /// This mob's body for soft entity pushing (feet at `pos`, sized to its species).
    pub(super) fn push_body(&self) -> push::Body {
        let s = def(self.kind).size;
        push::Body::new(self.pos, s.half_width, s.height)
    }

    /// Set this tick's soft entity-push velocity (the sum of the pushes from every
    /// entity it overlaps). It is applied — and consumed — on the next
    /// [`integrate`](Self::integrate), on top of locomotion, moving through the normal
    /// collision-resolved step so it can't push the mob through terrain.
    pub(super) fn set_push(&mut self, push: Vec3) {
        self.push = push;
    }

    /// Is the mob dead (ragdolling or done)? A dead mob can't be targeted or hurt.
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.death.is_some()
    }

    /// Has the death ragdoll finished, so the corpse should be removed from the world?
    #[inline]
    pub fn is_despawned(&self) -> bool {
        self.death.as_ref().is_some_and(Ragdoll::is_done)
    }

    /// Has this mob stayed beyond its category's despawn radius from the player long
    /// enough to be culled (always false for a category that never distance-despawns,
    /// e.g. a passive owl)? The manager drops such a mob from the live set without
    /// saving it — distinct from [`is_despawned`](Self::is_despawned), which is a
    /// finished death corpse.
    #[inline]
    pub fn is_distance_despawned(&self) -> bool {
        self.despawn_timer >= HOSTILE_DESPAWN_TICKS
    }

    /// Hurt-flash intensity in `[0, 1]` at `alpha` into the tick. The renderer fades the
    /// red tint by this. Applies while dying too (the flash from the killing blow), so a
    /// kill reads like any other hit; it decays to 0 over the start of the ragdoll.
    pub fn hurt_flash(&self, alpha: f32) -> f32 {
        let t = self.prev_hurt + (self.hurt_timer - self.prev_hurt) * alpha;
        (t / HURT_FLASH_SECS).clamp(0.0, 1.0)
    }

    /// The interpolated per-bone ragdoll pose (pivot position + orientation) at `alpha`,
    /// or `None` if the mob isn't ragdolling yet. The renderer builds each bone's pose
    /// as `T(pos)·R(rot)·T(-pivot)`.
    pub fn ragdoll_pose(&self, alpha: f32) -> Option<Vec<(Vec3, glam::Quat)>> {
        let rag = self.death.as_ref()?;
        if !rag.is_initialized() {
            return None;
        }
        Some(rag.pose(alpha))
    }

    /// Advance one game tick: snapshot the previous pose, let the brain pick a goal,
    /// have the navigator steer toward it, and integrate the kinematics. A dead mob
    /// runs no AI — only its death ragdoll advances.
    pub fn tick(
        &mut self,
        dt: f32,
        world: &World,
        player_pos: Vec3,
        idle_anims: &[IdleAnimMeta],
        skeleton: &Skeleton,
    ) {
        self.prev_pos = self.pos;
        self.prev_yaw = self.yaw;
        self.prev_anim_time = self.anim_time;
        self.prev_head_yaw = self.head_yaw;
        self.prev_head_pitch = self.head_pitch;
        self.prev_hurt = self.hurt_timer;

        let d = def(self.kind);

        // Dead: freeze the body (pos/yaw stay put — they're the ragdoll's `global`) and
        // advance only the physics ragdoll. No brain, no locomotion. The kill's red flash
        // still fades out over these first ticks.
        if self.death.is_some() {
            self.hurt_timer = (self.hurt_timer - dt).max(0.0);
            self.tick_ragdoll(dt, world, d, skeleton);
            return;
        }

        // Hurt flash + knockback stagger counts down on the fixed tick (frame-rate
        // independent).
        if self.hurt_timer > 0.0 {
            self.hurt_timer = (self.hurt_timer - dt).max(0.0);
        }

        // Distance-despawn: a hostile mob tallies ticks spent beyond its cull radius
        // from the player; once the tally reaches the threshold the manager removes it.
        // A passive species has no radius, so its timer never moves and it persists.
        if let Some(radius) = d.category.despawn_radius() {
            let far = (self.pos - player_pos).length_squared() >= radius * radius;
            self.despawn_timer = next_despawn_timer(self.despawn_timer, far);
        }

        let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
        // The model-aware box source for body collision (legs/top of a bbmodel block); the
        // cell-based `solid` above still drives navigation (foothold/pathfinding/ledge).
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        // The foothold cell the mob is standing in — robust to standing at a block
        // edge, where the cell under its centre overhangs into air. Without this the
        // start wouldn't be a foothold and pathfinding would bail, freezing the mob.
        let cell = path::standing_cell(self.pos, d.size.half_width, d.size.head_cells(), &solid)
            .unwrap_or_else(|| voxel_at(self.pos));
        let nav_idle = self.nav.is_idle();
        // On or in water — feet submerged, or resting on the surface (water just
        // below). Stays true while the mob floats at the surface; drives idle-animation
        // suppression.
        let feet_cell = voxel_at(self.pos);
        let in_water = water(feet_cell) || water(feet_cell - IVec3::Y);
        let decision = {
            let mut ctx = AiCtx {
                pos: self.pos,
                cell,
                yaw: self.yaw,
                head_height: d.size.height,
                world,
                player_pos,
                nav_idle,
                in_water,
                head: d.size.head_cells(),
                idle_anims,
                rng: &mut self.rng,
            };
            self.brain.decide(&mut ctx)
        };
        self.nav.update_goal(decision.goal, cell, world);
        let (wish, jump) = self.nav.follow(self.pos, self.on_ground);
        let water_flow = |c: IVec3| world.water_flow_dir_at(c.x, c.y, c.z);
        self.integrate_with_flow(dt, d, wish, jump, &boxes, &solid, &water, &water_flow);
        self.apply_expression(dt, d, &decision);
    }

    /// Advance the death ragdoll. On its first dead tick the ragdoll is initialised;
    /// thereafter it steps, colliding each bone-corner against the world's blocks (so the
    /// corpse can't pass through terrain and falls off edges). The mob's `pos`/`yaw` stay
    /// frozen — they're the ragdoll's model→world `global` transform.
    fn tick_ragdoll(&mut self, dt: f32, world: &World, d: &MobDef, skeleton: &Skeleton) {
        let vel = self.vel;
        let yaw = self.yaw;
        let pos = self.pos;
        let rag = self
            .death
            .as_mut()
            .expect("tick_ragdoll only runs when dead");
        if rag.is_initialized() {
            let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
            rag.step(dt, d.scale, pos, yaw, &solid);
        } else {
            rag.init(skeleton, d.scale, vel, yaw);
        }
    }

    /// Integrate one tick's kinematics: jump impulse, horizontal wish-velocity, water
    /// current, gravity, collision, and facing/anim. Takes `solid`/`water`/`water_flow`
    /// closures (not the world) so it's directly unit-testable against a stub. The mob
    /// faces its **wish** direction — where it wants to go — so it keeps facing forward
    /// even when pressed against a wall (where its actual velocity would be zero).
    fn integrate_with_flow(
        &mut self,
        dt: f32,
        d: &MobDef,
        wish: Vec3,
        jump: bool,
        boxes: &impl Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
        solid: &impl Fn(IVec3) -> bool,
        water: &impl Fn(IVec3) -> bool,
        water_flow: &impl Fn(IVec3) -> Vec3,
    ) {
        if jump && self.on_ground {
            self.vel.y = d.jump_speed;
            self.on_ground = false;
        }
        // During the knockback stagger the decaying knockback drives horizontal motion
        // (so a hit shoves the mob even against where it wants to go); otherwise the
        // wish velocity drives normal locomotion. Keeping knockback separate from `vel`
        // is why this overwrite can't wipe it.
        if self.hurt_timer > 0.0 {
            self.vel.x = self.knockback.x;
            self.vel.z = self.knockback.z;
            self.knockback *= KNOCKBACK_DAMP;
            self.moving = false;
        } else {
            self.vel.x = wish.x * d.walk_speed;
            self.vel.z = wish.z * d.walk_speed;
            self.moving = wish.length_squared() > 1e-6;
        }
        // Soft entity push: a velocity from being jostled by overlapping entities,
        // layered on top of locomotion (or knockback) so a crowded mob drifts apart
        // smoothly. Consumed each tick — the push pass re-derives it from the live
        // overlap — and left out of `moving`, so being shoved doesn't read as walking.
        self.vel.x += self.push.x;
        self.vel.z += self.push.z;
        self.push = Vec3::ZERO;

        // Water current: while standing in or swimming through flowing water, drift with
        // it — capped well below walk speed — so a mob caught in a river is carried
        // downstream instead of ignoring the flow. Unlike the player (whose velocity
        // carries momentum and eases into the current over several ticks), a mob rebuilds
        // its horizontal velocity from `wish` every tick, so the current contributes its
        // full drift in one tick (max step = the target speed) rather than a small accel
        // step that would never accumulate. It still never slows a mob already swimming
        // downstream faster than the current.
        let flow = flow_at_body(self.pos, d.size.height, water, water_flow);
        self.vel = add_flow_push(self.vel, flow, WATER_CURRENT_SPEED, WATER_CURRENT_SPEED);

        // Vertical. In water the mob always swims toward the surface (no jump key, so
        // it behaves like a player holding jump): vel eases up to `SWIM_RISE` until the
        // probe — a fraction up the body — clears the water; then it's airborne, so
        // gravity pulls it back, it re-enters, and rises again. The result is a bob
        // through the waterline, identical in feel to the player. Out of water: gravity.
        let probe = voxel_at(self.pos + Vec3::new(0.0, d.size.height * SWIM_PROBE_FRAC, 0.0));
        if water(probe) {
            // Climbing out: when steering toward a 1-block ledge it can get onto (and
            // not already falling back), a firm boost crests the waterline and lands it
            // on the block instead of hugging the shore forever — else the swim bob.
            let climbing_out = self.vel.y >= 0.0
                && wish.length_squared() > 1e-12
                && self.ledge_ahead(wish, d.size.half_width, solid);
            if climbing_out {
                self.vel.y = self.vel.y.max(SWIM_CLIMB);
            } else {
                self.vel.y = approach(self.vel.y, SWIM_RISE, SWIM_VACCEL * dt);
            }
        } else {
            self.vel.y += GRAVITY * dt;
        }
        // Body collision via the shared swept-AABB resolver (the same one the player and
        // dropped items use) against the block's REAL collision shape — so a mob stops at a
        // bbmodel block's legs/top, not its full cube. Navigation (foothold/pathfinding/
        // `ledge_ahead`) stays cell-based (`solid`): that's "is this cell an obstacle", a
        // separate concern from "does my body hit the shape".
        let hw = d.size.half_width;
        let min = [self.pos.x - hw, self.pos.y, self.pos.z - hw];
        let max = [self.pos.x + hw, self.pos.y + d.size.height, self.pos.z + hw];
        // A grounded mob auto-steps up a half-block ledge (a slab / a model block's low
        // edge) without jumping — same `STEP_HEIGHT` the player uses.
        let (moved, grounded, hit) = crate::collision::resolve_body(
            min,
            max,
            self.vel.to_array(),
            dt,
            crate::collision::STEP_HEIGHT,
            boxes,
        );
        self.pos += Vec3::from(moved);
        if hit[0] {
            self.vel.x = 0.0;
        }
        if hit[1] {
            self.vel.y = 0.0;
        }
        if hit[2] {
            self.vel.z = 0.0;
        }
        self.on_ground = grounded;
        if grounded && self.vel.y < 0.0 {
            self.vel.y = 0.0;
        }

        if self.moving {
            let target = heading_yaw(wish);
            self.yaw = turn_toward(self.yaw, target, d.turn_rate * dt);
        }
    }

    /// [`integrate_with_flow`](Self::integrate_with_flow) in still water — the unit tests
    /// drive the kinematics against a stub world with no currents.
    #[cfg(test)]
    fn integrate(
        &mut self,
        dt: f32,
        d: &MobDef,
        wish: Vec3,
        jump: bool,
        solid: &impl Fn(IVec3) -> bool,
        water: &impl Fn(IVec3) -> bool,
    ) {
        self.integrate_with_flow(dt, d, wish, jump, &boxes_of(solid), solid, water, &|_| {
            Vec3::ZERO
        });
    }

    /// Apply the tick's expressive decision: choose + advance the active animation
    /// (walk while moving, an `idle_*` if one was requested, else the neutral rest
    /// pose), and ease the head toward the head-look target (recentring when there's
    /// none — e.g. while walking).
    fn apply_expression(&mut self, dt: f32, d: &MobDef, decision: &BehaviorOutput) {
        // An idle animation only plays while the mob isn't walking.
        self.idle_anim = if self.moving {
            None
        } else {
            decision.idle_anim
        };

        // Pick the active animation; reset its phase whenever it changes.
        let kind = if self.moving {
            AnimKind::Walk
        } else if let Some(i) = self.idle_anim {
            AnimKind::Idle(i)
        } else {
            AnimKind::Rest
        };
        if kind != self.anim_kind {
            self.anim_kind = kind;
            self.anim_time = 0.0;
            self.prev_anim_time = 0.0;
        }
        // Advance the active animation: walk at the species' rate, idle at its natural
        // rate, rest frozen (the renderer shows the static rest pose).
        match kind {
            AnimKind::Walk => self.anim_time += d.walk_anim_rate * dt,
            AnimKind::Idle(_) => self.anim_time += dt,
            AnimKind::Rest => {}
        }

        // Head-look: ease toward the requested orientation, or recentre when none.
        let (target_yaw, target_pitch) = match decision.head_look {
            Some(h) => (h.yaw, h.pitch),
            None => (0.0, 0.0),
        };
        let step = HEAD_TURN_RATE * dt;
        self.head_yaw = turn_toward(self.head_yaw, target_yaw, step);
        self.head_pitch = approach(self.head_pitch, target_pitch, step);
    }

    /// Move along each axis in turn, resolving against solid cells; returns whether
    /// the mob is resting on the ground after the move. Mirrors the dropped-item
    /// integrator, sized to the mob's AABB.
    /// Is there a 1-block ledge to climb onto just ahead in `dir` (horizontal)? True
    /// when the cell just beyond the body is solid at the feet (or one above) with open
    /// space directly above it — a single step, not a taller wall (so swimming into a
    /// cliff face won't lift the mob up it). Mirrors the player's climb-out probe.
    fn ledge_ahead(&self, dir: Vec3, half_width: f32, solid: &impl Fn(IVec3) -> bool) -> bool {
        let d = Vec3::new(dir.x, 0.0, dir.z);
        if d.length_squared() <= 1e-12 {
            return false;
        }
        let d = d.normalize_or_zero();
        // A cell just beyond the body's footprint in the move direction.
        let fx = (self.pos.x + d.x * (half_width + 0.2)).floor() as i32;
        let fz = (self.pos.z + d.z * (half_width + 0.2)).floor() as i32;
        let base = self.pos.y.floor() as i32;
        // A step at feet level, or one block above (so the boost engages from ~a block
        // below the ledge top, giving runway to crest it).
        let step_at = |y: i32| solid(IVec3::new(fx, y, fz)) && !solid(IVec3::new(fx, y + 1, fz));
        step_at(base) || step_at(base + 1)
    }

    #[cfg(test)]
    pub(super) fn on_ground(&self) -> bool {
        self.on_ground
    }
}

/// The yaw that faces the horizontal component of `v`. The model faces `-Z` at
/// `yaw = 0` (the renderer applies `rotation_y(yaw)`), so heading `(vx, vz)` maps to
/// `atan2(-vx, -vz)`.
fn heading_yaw(v: Vec3) -> f32 {
    (-v.x).atan2(-v.z)
}

/// Turn `yaw` toward `target` by at most `max_step`, along the shortest arc.
fn turn_toward(yaw: f32, target: f32, max_step: f32) -> f32 {
    let delta = wrap_angle(target - yaw);
    let step = max_step.min(delta.abs());
    wrap_angle(yaw + step * delta.signum())
}

/// Wrap an angle into `[-PI, PI]`.
fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Move `cur` toward `target` by at most `step` (linear, no wrapping).
fn approach(cur: f32, target: f32, step: f32) -> f32 {
    cur + (target - cur).clamp(-step, step)
}

/// Advance a mob's distance-despawn timer by one tick: count up while it is `far` from
/// the player, and reset to zero the instant a player is near again, so only a
/// *sustained* absence culls the mob. Saturating, so a very long absence can't wrap.
fn next_despawn_timer(prev: u32, far: bool) -> u32 {
    if far {
        prev.saturating_add(1)
    } else {
        0
    }
}

/// The water-flow direction acting on a mob whose feet are at `pos`: the current at the
/// swim probe (a fraction up the body, where the mob is submerged enough to swim), else
/// the current at the feet cell (so a mob wading in a shallow flowing film is still
/// nudged), else zero when no water touches it.
fn flow_at_body(
    pos: Vec3,
    height: f32,
    water: &impl Fn(IVec3) -> bool,
    water_flow: &impl Fn(IVec3) -> Vec3,
) -> Vec3 {
    let swim = voxel_at(pos + Vec3::new(0.0, height * SWIM_PROBE_FRAC, 0.0));
    if water(swim) {
        return water_flow(swim);
    }
    let feet = voxel_at(pos);
    if water(feet) {
        return water_flow(feet);
    }
    Vec3::ZERO
}

/// Add a capped push along the water-flow direction `dir` without slowing a body that
/// already drifts at least `target_speed` along it. Mirrors the player's and dropped
/// item's current handling, so every entity rides a current the same way. Horizontal
/// only — `vel.y` is untouched.
fn add_flow_push(vel: Vec3, dir: Vec3, target_speed: f32, max_delta: f32) -> Vec3 {
    let len_sq = dir.x * dir.x + dir.z * dir.z;
    if len_sq <= 1e-12 || target_speed <= 0.0 || max_delta <= 0.0 {
        return vel;
    }
    let inv_len = len_sq.sqrt().recip();
    let nx = dir.x * inv_len;
    let nz = dir.z * inv_len;
    let along = vel.x * nx + vel.z * nz;
    let add = (target_speed - along).clamp(0.0, max_delta);
    Vec3::new(vel.x + nx * add, vel.y, vel.z + nz * add)
}

/// Bridge a cell-solid bool stub into the shared collision box source (a full cube per
/// solid cell), so the kinematics tests keep driving body physics with a simple `solid`
/// predicate while it routes through the same `collision::resolve_body` as production.
#[cfg(test)]
fn boxes_of(
    solid: &impl Fn(IVec3) -> bool,
) -> impl Fn(i32, i32, i32) -> &'static [crate::block::Aabb] + '_ {
    move |x, y, z| {
        if solid(IVec3::new(x, y, z)) {
            crate::block::Block::Stone.collision_boxes()
        } else {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;

    fn floor_at_zero(p: IVec3) -> bool {
        p.y < 0
    }

    #[test]
    fn despawn_timer_counts_far_ticks_and_resets_when_near() {
        // The distance-despawn tally only grows while the mob is far, and a single near
        // tick wipes it — so only a sustained absence reaches the cull threshold.
        let mut t = 0;
        for _ in 0..5 {
            t = next_despawn_timer(t, true);
        }
        assert_eq!(t, 5, "counts up while far");
        t = next_despawn_timer(t, false);
        assert_eq!(t, 0, "a near tick resets the tally");

        // It takes a full uninterrupted run of HOSTILE_DESPAWN_TICKS to cross the line.
        let mut t = 0;
        for _ in 0..HOSTILE_DESPAWN_TICKS {
            assert!(
                t < HOSTILE_DESPAWN_TICKS,
                "below threshold until the last tick"
            );
            t = next_despawn_timer(t, true);
        }
        assert!(t >= HOSTILE_DESPAWN_TICKS, "reaches the cull threshold");
    }

    fn owl_def() -> &'static MobDef {
        def(Mob::Owl)
    }

    #[test]
    fn gravity_settles_the_mob_on_the_floor() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
        for _ in 0..600 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            owl.pos.y >= -1e-3,
            "mob fell through the floor: {}",
            owl.pos.y
        );
        assert!(owl.pos.y < 0.05, "mob rests on the floor: {}", owl.pos.y);
        assert!(owl.on_ground());
    }

    #[test]
    fn mob_body_rests_on_an_inset_block_top_not_the_cell_top() {
        // Model-aware body collision: a mob settling onto an INSET block (a chest, top at
        // 14/16) rests its feet on that real top, not the full-cube cell top (y = 1). The
        // mob body now collides through the shared `collision_boxes_at` shape (nav stays
        // cell-based, but that's a separate concern).
        let chest = crate::block::Block::Chest.collision_boxes();
        let chest_top = chest.iter().map(|b| b.max[1]).fold(0.0, f32::max);
        assert!(
            chest_top < 1.0,
            "the chest box must be inset (top {chest_top})"
        );
        let boxes = |_x: i32, y: i32, _z: i32| if y == 0 { chest } else { &[][..] };
        let solid = |c: IVec3| c.y == 0; // nav sees the chest cell as a unit obstacle
        let dry = |_: IVec3| false;
        let still = |_: IVec3| Vec3::ZERO;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
        for _ in 0..600 {
            owl.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &boxes,
                &solid,
                &dry,
                &still,
            );
        }
        assert!(owl.on_ground(), "mob should be grounded on the chest");
        assert!(
            (owl.pos.y - chest_top).abs() < 0.02,
            "mob feet should rest on the chest top {chest_top}, got {}",
            owl.pos.y
        );
    }

    #[test]
    fn grounded_mob_auto_steps_up_a_half_block() {
        // A grounded mob walking into a 0.5-tall ledge auto-climbs it (same STEP_HEIGHT as
        // the player), without needing a jump.
        let half_step = |x: i32, y: i32, _z: i32| -> &'static [crate::block::Aabb] {
            if y == 0 {
                Block::Stone.collision_boxes()
            } else if y == 1 && x >= 1 {
                &[crate::block::Aabb {
                    min: [0.0, 0.0, 0.0],
                    max: [1.0, 0.5, 1.0],
                }]
            } else {
                &[]
            }
        };
        let solid = |c: IVec3| c.y == 0 || (c.y == 1 && c.x >= 1); // nav obstacle
        let dry = |_: IVec3| false;
        let still = |_: IVec3| Vec3::ZERO;
        let wish = Vec3::new(1.0, 0.0, 0.0);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        for _ in 0..180 {
            owl.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                wish,
                false,
                &half_step,
                &solid,
                &dry,
                &still,
            );
        }
        assert!(owl.pos.x > 1.2, "mob steps onto the ledge: x={}", owl.pos.x);
        assert!(
            owl.pos.y > 1.4,
            "mob rises onto the 0.5 ledge top: y={}",
            owl.pos.y
        );
    }

    #[test]
    fn wish_direction_drives_horizontal_motion_and_facing() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        // Settle on the ground first.
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        let x0 = owl.pos.x;
        for _ in 0..30 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::new(1.0, 0.0, 0.0),
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            owl.pos.x > x0 + 0.3,
            "wish +X should move the mob: {} -> {}",
            x0,
            owl.pos.x
        );
        assert!(owl.moving, "moving flag set while walking");
        // Faces +X: heading_yaw((+,0,0)) = atan2(-1, 0) = -PI/2.
        assert!(
            (wrap_angle(owl.yaw - (-PI / 2.0))).abs() < 0.2,
            "turns to face travel: {}",
            owl.yaw
        );
    }

    #[test]
    fn expression_advances_walk_and_eases_the_head() {
        use super::super::brain::HeadLook;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        // A walking tick (integrate sets `moving`), then expression advances the walk.
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
            &|_| false,
        );
        owl.apply_expression(1.0 / 60.0, owl_def(), &BehaviorOutput::default());
        let a1 = owl.anim_time;
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
            &|_| false,
        );
        owl.apply_expression(1.0 / 60.0, owl_def(), &BehaviorOutput::default());
        assert!(
            owl.anim_time > a1,
            "walk cycle keeps advancing: {a1} -> {}",
            owl.anim_time
        );

        // Head eases toward a look target, then recentres when there's none.
        let look = BehaviorOutput {
            head_look: Some(HeadLook {
                yaw: 1.0,
                pitch: 0.3,
            }),
            ..Default::default()
        };
        for _ in 0..120 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &floor_at_zero,
                &|_| false,
            );
            owl.apply_expression(1.0 / 60.0, owl_def(), &look);
        }
        assert!(
            (owl.head_yaw - 1.0).abs() < 0.05,
            "head reaches yaw target: {}",
            owl.head_yaw
        );
        assert!(
            (owl.head_pitch - 0.3).abs() < 0.05,
            "head reaches pitch target: {}",
            owl.head_pitch
        );
        for _ in 0..120 {
            owl.apply_expression(1.0 / 60.0, owl_def(), &BehaviorOutput::default());
        }
        assert!(
            owl.head_yaw.abs() < 0.05,
            "head recentres in yaw: {}",
            owl.head_yaw
        );
        assert!(
            owl.head_pitch.abs() < 0.05,
            "head recentres in pitch: {}",
            owl.head_pitch
        );
    }

    #[test]
    fn jump_impulse_lifts_a_grounded_mob() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        assert!(owl.on_ground());
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            true,
            &floor_at_zero,
            &|_| false,
        );
        assert!(!owl.on_ground(), "jump leaves the ground");
        assert!(owl.pos.y > 0.0, "jump raises the mob");
    }

    #[test]
    fn idle_mob_is_not_moving() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        for _ in 0..10 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            !owl.moving,
            "a still mob reports not moving (renders the rest pose)"
        );
    }

    #[test]
    fn hurt_reduces_health_and_dies_at_zero() {
        // A 4-health owl: three 1-damage hits don't kill; the fourth does.
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        let from = Vec3::new(5.0, 0.0, 0.5);
        assert!(!owl.hurt(1.0, from));
        assert!(!owl.hurt(1.0, from));
        assert!(!owl.hurt(1.0, from));
        assert!(!owl.is_dead(), "still alive at 1 health");
        assert!(owl.hurt(1.0, from), "the lethal hit reports true");
        assert!(owl.is_dead(), "dead at 0 health");
    }

    #[test]
    fn a_dead_mob_ignores_further_damage() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(
            owl.hurt(100.0, Vec3::new(5.0, 0.0, 0.5)),
            "one big hit kills"
        );
        // A corpse takes no more damage and reports no further lethal hits.
        assert!(!owl.hurt(100.0, Vec3::new(5.0, 0.0, 0.5)));
        assert!(owl.is_dead());
    }

    #[test]
    fn knockback_pushes_away_and_overrides_the_wish() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        // Settle on the floor first.
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
            false
        });
        let x0 = owl.pos.x;
        // Hit from the +X side → knockback toward -X. This is the key invariant: the
        // knockback survives `integrate`'s per-tick wish-velocity overwrite.
        assert!(!owl.hurt(1.0, Vec3::new(5.0, 0.0, 0.5)));
        // Wish toward +X (toward the attacker); the knockback must win during the stagger.
        for _ in 0..4 {
            owl.integrate(
                0.05,
                owl_def(),
                Vec3::new(1.0, 0.0, 0.0),
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            owl.pos.x < x0 - 0.05,
            "knocked back -X despite wishing +X: {x0} -> {}",
            owl.pos.x
        );
        assert!(!owl.moving, "a staggered mob doesn't read as walking");
    }

    #[test]
    fn every_hit_flashes_red_including_the_kill() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.hurt(1.0, Vec3::new(5.0, 0.0, 0.5));
        assert!(owl.hurt_flash(1.0) > 0.0, "a non-lethal hit flashes red");

        // The killing blow flashes red too (so it looks like any other hit).
        let mut dead = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(dead.hurt(100.0, Vec3::new(5.0, 0.0, 0.5)));
        assert!(
            dead.hurt_flash(1.0) > 0.0,
            "the kill flashes red like a normal hit"
        );
        assert!(
            dead.ragdoll_pose(0.5).is_none(),
            "ragdoll pose is None until a dead tick inits it"
        );
    }

    #[test]
    fn a_submerged_mob_swims_up_instead_of_sinking() {
        // Solid bed below y==0, water filling y in 0..=5. Start the mob submerged at
        // y==1: buoyancy should lift it over a few ticks (gravity alone would sink it).
        let solid = |c: IVec3| c.y < 0;
        let water = |c: IVec3| (0..=5).contains(&c.y);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        let y0 = owl.pos.y;
        for _ in 0..20 {
            owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &solid, &water);
        }
        assert!(
            owl.pos.y > y0,
            "a submerged mob rises toward the surface: {y0} -> {}",
            owl.pos.y
        );
    }

    #[test]
    fn a_mob_bobs_up_and_down_through_the_water_surface_like_the_player() {
        // Water fills y in 0..=5 (surface at y==6) over a solid bed at y<0. The mob
        // swims up, breaks the surface, gravity pulls it back, it re-enters and rises
        // again — a real bob through the waterline (not a dead float, not a wiggle that
        // never re-enters). Run the real 20 TPS step.
        let solid = |c: IVec3| c.y < 0;
        let water = |c: IVec3| (0..=5).contains(&c.y);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        // Let it rise to the surface and get into the bob.
        for _ in 0..100 {
            owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &solid, &water);
        }
        // Over the next couple of seconds it must move both up (swim) and down
        // (gravity), and stay in a sane band around the surface.
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let (mut went_up, mut went_down) = (false, false);
        for _ in 0..120 {
            let before = owl.pos.y;
            owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &solid, &water);
            let dy = owl.pos.y - before;
            went_up |= dy > 0.01;
            went_down |= dy < -0.01;
            lo = lo.min(owl.pos.y);
            hi = hi.max(owl.pos.y);
        }
        assert!(
            went_up && went_down,
            "bobs both up and down (up {went_up}, down {went_down})"
        );
        assert!(hi > 5.5, "rises up to/through the surface: hi {hi}");
        assert!(
            (4.0..=7.0).contains(&lo) && (4.0..=7.0).contains(&hi),
            "stays at the waterline: {lo}..{hi}"
        );
    }

    #[test]
    fn a_swimming_mob_climbs_out_onto_an_adjacent_ledge() {
        // A shore the climb-boost can actually clear: water (cells y in 0..SURFACE) over a
        // bed at y<0, with land at x>=1 whose top is AT the waterline. The swim climb-boost
        // (`SWIM_CLIMB`, fired by `ledge_ahead`) lifts the mob's feet just over the surface
        // so it steps out onto the land instead of hugging the shore forever. How high the
        // boost reaches depends on the (tunable) swim constants, so the land is kept at the
        // waterline and the checks derive from the owl's own size + this geometry — no swim
        // numbers are baked in. (The original test hard-coded a 1-block ledge, which needs
        // a far stronger boost than the tuned `SWIM_CLIMB` and so never passed.)
        const SURFACE: i32 = 4; // top of the water (and of the land it climbs onto)
        const SHORE: f32 = 1.0; // land starts at world x = 1
        let solid = |c: IVec3| c.y < 0 || (c.x >= 1 && c.y < SURFACE);
        let water = |c: IVec3| c.x <= 0 && (0..SURFACE).contains(&c.y);
        let half = owl_def().size.half_width;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        for _ in 0..300 {
            owl.integrate(
                0.05,
                owl_def(),
                Vec3::new(1.0, 0.0, 0.0),
                false,
                &solid,
                &water,
            );
        }
        assert!(
            owl.on_ground(),
            "settled on the land, not still bobbing in the water: y {}",
            owl.pos.y
        );
        assert!(
            owl.pos.y >= SURFACE as f32 - 0.05,
            "rests up at the land surface, out of the water: y {}",
            owl.pos.y
        );
        assert!(
            owl.pos.x + half > SHORE,
            "climbed past the shore onto the land: x {}",
            owl.pos.x
        );
    }

    #[test]
    fn a_mob_in_flowing_water_is_carried_downstream() {
        // Water fills y in 0..=5 over a solid bed at y<0, with a current heading +X
        // everywhere. A mob sitting in it with no wish to move must still drift
        // downstream — like the player and dropped items do.
        let solid = |c: IVec3| c.y < 0;
        let water = |c: IVec3| (0..=5).contains(&c.y);
        let flow = |_: IVec3| Vec3::new(1.0, 0.0, 0.0);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        let x0 = owl.pos.x;
        for _ in 0..60 {
            owl.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &boxes_of(&solid),
                &solid,
                &water,
                &flow,
            );
        }
        assert!(
            owl.pos.x > x0 + 0.3,
            "the current carries the mob downstream: {x0} -> {}",
            owl.pos.x
        );

        // Still water (no current) leaves an idle mob where it is — proving it's the flow
        // doing the carrying, not stray drift.
        let still = |_: IVec3| Vec3::ZERO;
        let mut calm = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        for _ in 0..60 {
            calm.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &boxes_of(&solid),
                &solid,
                &water,
                &still,
            );
        }
        assert!(
            (calm.pos.x - 0.5).abs() < 1e-3,
            "no current → no horizontal drift: x {}",
            calm.pos.x
        );
    }
}
