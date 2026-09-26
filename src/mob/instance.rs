//! A live mob instance: its public render pose plus the cohesive state
//! components the tick's phases work on (see [`parts`]).
//!
//! Everything physical a mob does — gravity, axis-resolved block collision, the jump
//! impulse, turning to face travel, advancing the walk cycle — lives here once and
//! is shared by every species; a species differs only by its [`MobDef`](super::MobDef)
//! stats and its brain's behaviors. One tick is one **game tick** (20 TPS), run
//! by the manager as a sequence of phases across the whole population (see
//! [`tick`]): the brain picks a goal, the navigator turns it into a
//! wish-direction + jump, and the kinematics integrate it. The previous tick's
//! pose is snapshotted each tick so the renderer can interpolate between ticks
//! for smooth motion at any frame rate.
//!
//! The `impl Instance` blocks are split by concern across sibling files —
//! [`kinematics`](super::kinematics) (locomotion integration and fall
//! bookkeeping), [`damage`](super::damage) (damage intake and the death
//! lifecycle), [`anim`](super::anim) (expression + named animation layers)
//! and [`tick`] (the per-tick phases) — the `world::store` pattern. This
//! file keeps the struct, spawn, and the accessors.

use std::collections::BTreeMap;
use std::sync::Arc;

use petramond_math::math::{IVec3, Tilt};

// Re-exported so `mob::instance::AnimLayer` consumers (the manager's
// anim-state readback) keep their path while the type lives with the
// animation impl.
pub use super::anim::AnimLayer;
use super::nav::Navigator;
use super::{def, EntityRef, Mob, MobRng, MobTagValue, DEFAULT_DAMAGE_FLASH_SECS};

mod parts;
pub(super) mod tick;

use parts::{Combat, Confinement, Interp, Mind, Motion, Presentation};
pub(super) use tick::{Begun, Footing, MobTickCtx, MotionStart, SpeciesMeta};

/// Default duration used to normalize hurt-flash intensity. Individual feedback
/// components may start shorter or longer flash timers.
const HURT_FLASH_SECS: f32 = DEFAULT_DAMAGE_FLASH_SECS;

/// Hurt-flash intensity in `[0, 1]` from a previous/current hurt-timer pair at
/// `alpha` into the tick — the ONE derivation, shared by the live instance
/// ([`Instance::hurt_flash`]) and the client's replicated-store presentation
/// path (which interpolates consecutive replicated timers).
pub fn hurt_flash01(prev: f32, curr: f32, alpha: f32) -> f32 {
    let t = prev + (curr - prev) * alpha;
    (t / HURT_FLASH_SECS).clamp(0.0, 1.0)
}

/// A live mob. The render-facing pose (`pos`/`yaw`/`tilt`/`anim_time`/`moving`/
/// `idle_anim`/head/light) is public for the scene adapter; everything else
/// lives in per-concern components private to the `mob` module (shared with
/// the sibling `impl Instance` files).
pub struct Instance {
    /// Stable session identity for this live mob. Unlike its storage slot,
    /// this does not change when the manager `swap_remove`s another mob.
    pub(super) id: u64,
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    /// Body tilt inside the yaw. Level for every body the engine moves
    /// itself: only a kinematic placement (`set_kinematic`) tilts a body,
    /// and one the engine takes back eases level again (`level_body`).
    pub tilt: Tilt,
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
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: petramond_world::light::BlockLight6,
    /// Last tick's pose, for render interpolation.
    pub(super) interp: Interp,
    /// The body's kinematic state and this tick's mod intents.
    pub(super) motion: Motion,
    /// Vitality feedback, death, the latched strike and retaliation memory.
    pub(super) combat: Combat,
    /// Brain, navigator and the decision state they carry.
    pub(super) mind: Mind,
    /// The periodic confined verdict and its cadence.
    confinement: Confinement,
    /// Mod-controlled presentation and the expression clock's state.
    pub(super) presentation: Presentation,
    /// Engine- and mod-owned tags attached to this mob instance, seeded at
    /// spawn from the species row's spawn tags ([`MobDef::tags`]). The engine
    /// reserves the `petramond:` namespace (see [`super::tags`]) — health and
    /// shear regrowth live HERE, not in dedicated fields; mods may invent
    /// `mod_id:` keys. Persisted (see [`super::SavedMob`]).
    /// Copy-on-write: the per-tick AI snapshot and scripted-node requests
    /// share the map by `Arc` clone, so a write (`tags_mut`) only clones the
    /// contents when one of them is holding the previous state.
    tags: Arc<BTreeMap<String, MobTagValue>>,
    /// Transient body conditions and fluid contact; dead bodies retain their
    /// last condition stages for presentation.
    exposure: petramond_world::exposure::BodyExposure,
    /// True once this mob is beyond its row-level despawn radius this tick. The manager
    /// culls it at the end of the tick. Never persisted.
    pub(super) distance_despawned: bool,
    pub(super) rng: MobRng,
    /// Carried item storage, sized by the row's `container_slots`. Persisted
    /// with the mob; its contents scatter when the mob leaves the world any
    /// way other than a save.
    container: petramond_world::container::Container,
    /// The dig this mob is driven through, a tick at a time. Transient: a
    /// reloaded mob starts its dig over.
    dig: DrivenDig,
}

/// A dig driven from outside, a tick at a time, on the mining clock a
/// player's held button runs. One that misses a tick has stopped.
#[derive(Clone, Debug, Default)]
pub struct DrivenDig {
    mining: petramond_world::mining::MiningState,
    /// The last tick the dig advanced.
    tick: u64,
}

/// What one tick of a driven dig came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DigStep {
    /// Still digging, this far through (0..1).
    Digging(f32),
    /// The block's whole break time has accrued.
    Done(petramond_world::mining::BreakEvent),
}

impl Instance {
    /// Spawn a mob of `kind` at `pos` (feet) facing `yaw`. `seed` makes its AI
    /// deterministic and distinct per mob.
    pub fn new(kind: Mob, pos: petramond_math::world_pos::WorldPos, yaw: f32, seed: u64) -> Self {
        let d = def(kind);
        Instance {
            id: seed,
            kind,
            pos,
            yaw,
            tilt: Tilt::LEVEL,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            skylight: 63,
            blocklight: petramond_world::light::BlockLight6::DARK,
            interp: Interp::at_rest(pos, yaw),
            motion: Motion::at_rest(pos.y),
            combat: Combat::default(),
            mind: Mind {
                brain: super::build_brain(d),
                nav: Navigator::new(d.size.head_cells(), d.size.half_width, d.size.height)
                    .tolerating(d.tolerates.blocks)
                    .with_tuning(d.nav),
                unstick: Default::default(),
                current_target: None,
                held_decision: Default::default(),
                contacts: Vec::new(),
            },
            confinement: Confinement::new(seed),
            presentation: Presentation::default(),
            tags: Arc::new(d.tags.clone()),
            exposure: petramond_world::exposure::BodyExposure::new(d.tolerates),
            distance_despawned: false,
            rng: MobRng::new(seed),
            container: petramond_world::container::Container::with_len(d.container_slots),
            dig: DrivenDig::default(),
        }
    }

    /// Conditions and fluid contact, bound to the species' tolerance.
    #[inline]
    pub fn exposure(&self) -> &petramond_world::exposure::BodyExposure {
        &self.exposure
    }

    /// Every condition grant on this body goes through here.
    #[inline]
    pub fn exposure_mut(&mut self) -> &mut petramond_world::exposure::BodyExposure {
        &mut self.exposure
    }

    /// Stable session identity for this live mob. This is the value exposed to
    /// mods; storage slots stay private to the manager.
    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether this mob's navigator has a route search suspended for budget,
    /// waiting to continue.
    #[inline]
    pub(super) fn nav_search_waiting(&self) -> bool {
        self.mind.nav.search_waiting()
    }

    /// Take the melee strike the brain latched this tick, if any — the manager
    /// drains it after the tick into a [`MobAttack`](super::MobAttack) for
    /// `Game` to apply.
    #[inline]
    pub(super) fn take_attack(&mut self) -> Option<super::brain::AttackIntent> {
        self.combat.attack.take()
    }

    /// The active particle-emitter bundle ids, sorted (catalog session ids —
    /// `petramond_world::particle_emitters`).
    #[inline]
    pub fn active_emitters(&self) -> &[u8] {
        &self.presentation.active_emitters
    }

    /// Toggle one emitter bundle by catalog id (the manager resolves keys —
    /// see [`super::Mobs::set_mob_emitter`]). Returns `false` only when an
    /// activation would exceed [`super::MAX_ACTIVE_MOB_EMITTERS`].
    pub(super) fn set_emitter_active(&mut self, id: u8, active: bool) -> bool {
        let emitters = &mut self.presentation.active_emitters;
        match (emitters.binary_search(&id), active) {
            (Ok(_), true) | (Err(_), false) => true,
            (Ok(at), false) => {
                emitters.remove(at);
                true
            }
            (Err(at), true) => {
                if emitters.len() >= super::MAX_ACTIVE_MOB_EMITTERS {
                    return false;
                }
                emitters.insert(at, id);
                true
            }
        }
    }

    /// The mob's centre-square body projection. Systems that need the complete
    /// long-body footprint use `mob::body_geometry` instead.
    pub fn aabb(&self) -> ([f64; 3], [f64; 3]) {
        self.body().aabb()
    }

    /// This mob's gameplay body (feet at `pos`, sized to its species).
    pub(super) fn body(&self) -> petramond_world::body::Body {
        let s = def(self.kind).size;
        petramond_world::body::Body::new(self.pos, s.half_width, s.height)
    }

    /// Replace this mob's touch-contact record (see [`Mind::contacts`]) — the
    /// manager's push pass writes it from the same overlap tests that compute
    /// the pushes. The buffer is reused; nothing allocates on a quiet tick.
    pub(super) fn set_contacts(&mut self, contacts: impl IntoIterator<Item = EntityRef>) {
        self.mind.contacts.clear();
        self.mind.contacts.extend(contacts);
    }

    /// The entities whose bodies overlapped this mob last tick.
    #[inline]
    pub fn contacts(&self) -> &[EntityRef] {
        &self.mind.contacts
    }

    /// Is the mob currently shorn (its coat still regrowing)? The renderer hides the
    /// model's coat cubes while this holds. Reads the `petramond:shear_regrow` tag —
    /// present and positive = shorn; absent = fully coated.
    #[inline]
    pub fn is_shorn(&self) -> bool {
        self.tag_int(super::tags::SHEAR_REGROW) > 0
    }

    /// The `Int` tag under `key`, or `0` when absent / another type — the
    /// engine's read shape for its own countdown tags.
    #[inline]
    fn tag_int(&self, key: &str) -> i64 {
        self.tags
            .get(key)
            .and_then(MobTagValue::as_int)
            .unwrap_or(0)
    }

    /// The mob's tag map (engine- and mod-owned key/value pairs).
    #[inline]
    pub fn tags(&self) -> &BTreeMap<String, MobTagValue> {
        &self.tags
    }

    /// The mob's carried item slots.
    #[inline]
    pub fn container(&self) -> &petramond_world::container::Container {
        &self.container
    }

    #[inline]
    pub fn container_mut(&mut self) -> &mut petramond_world::container::Container {
        &mut self.container
    }

    /// One tick (`now`) of digging `block` at `pos` with `tool`. Asked twice
    /// in a tick, the second advances nothing.
    pub fn advance_dig(
        &mut self,
        now: u64,
        pos: IVec3,
        block: petramond_world::block::Block,
        tool: Option<petramond_world::item::Tool>,
    ) -> DigStep {
        use petramond_world::mining;
        if self.dig.tick + 1 < now {
            self.dig.mining.reset();
        }
        let dt = if self.dig.tick == now {
            0.0
        } else {
            crate::events::tick::TICK_DT
        };
        self.dig.tick = now;
        match self.dig.mining.advance(dt, pos, block, tool) {
            Some(done) => DigStep::Done(done),
            None => {
                let elapsed = self.dig.mining.progress().map_or(0.0, |(_, t)| t);
                DigStep::Digging(elapsed / mining::break_time(block, tool).max(f32::EPSILON))
            }
        }
    }

    /// The cell being dug and its crack stage, while the dig is running.
    pub fn dig_overlay(&self, now: u64) -> Option<(IVec3, u8)> {
        (self.dig.tick + 1 >= now)
            .then(|| self.dig.mining.overlay())
            .flatten()
    }

    /// The items drawn in the main and off hands.
    pub fn held(&self) -> [Option<petramond_world::item::ItemType>; 2] {
        self.presentation.held
    }

    pub fn draw(&self) -> &crate::world::draw::BodyDraw {
        &self.presentation.draw
    }

    pub fn set_draw(&mut self, draw: crate::world::draw::BodyDraw) {
        if self.presentation.draw != draw {
            self.presentation.draw = draw;
        }
    }

    pub fn set_held(&mut self, held: [Option<petramond_world::item::ItemType>; 2]) {
        self.presentation.held = held;
    }

    /// Restore saved slots, grown to the row's declared capacity; saved slots
    /// beyond a shrunken capacity are kept rather than thrown away.
    pub fn restore_container(&mut self, mut saved: petramond_world::container::Container) {
        let declared = def(self.kind).container_slots;
        if saved.slots.len() < declared {
            saved.slots.resize(declared, None);
        }
        self.container = saved;
    }

    /// Empty the carried slots, returning every stack they held.
    pub fn take_container_items(&mut self) -> Vec<petramond_world::item::ItemStack> {
        self.container
            .slots
            .iter_mut()
            .filter_map(Option::take)
            .collect()
    }

    /// The mob's tag map behind its shared handle — the AI snapshot's copy is
    /// this cheap `Arc` clone, not a per-tick deep clone of the whole map.
    #[inline]
    pub(super) fn tags_shared(&self) -> Arc<BTreeMap<String, MobTagValue>> {
        Arc::clone(&self.tags)
    }

    /// Mutable access to the mob's tag map, for HostCalls and the reload path.
    /// Clones the contents when a shared snapshot still holds them (see the
    /// field docs) — writers should check-and-skip no-op writes.
    #[inline]
    pub(super) fn tags_mut(&mut self) -> &mut BTreeMap<String, MobTagValue> {
        Arc::make_mut(&mut self.tags)
    }

    /// Whether the mob is currently confined (captive / penned).
    #[inline]
    pub fn is_confined(&self) -> bool {
        self.tags
            .get(super::tags::CONFINED)
            .and_then(MobTagValue::as_bool)
            == Some(true)
    }

    /// Overlay saved tags onto the spawn-seeded map (reload path): saved
    /// values win per-key, while spawn tags a saved record predates stay —
    /// so a species gaining a new spawn tag reaches previously saved mobs.
    pub(super) fn overlay_tags(&mut self, saved: BTreeMap<String, MobTagValue>) {
        let tags = self.tags_mut();
        for (k, v) in saved {
            tags.insert(k, v);
        }
    }

    /// Shear this mob: roll how many of its [`ShearSpec`](super::ShearSpec) drop it
    /// yields and start the regrow countdown (the `petramond:shear_regrow` tag).
    /// `None` when the species can't be shorn, the coat is still regrowing, or the
    /// mob is dead.
    pub(super) fn shear(&mut self) -> Option<u8> {
        // The shared coat gate — the rule the client's shear prediction runs
        // against the replicated row.
        if !crate::rules::item_use::can_shear_coat(self.kind, self.combat.death.is_dead(), self.is_shorn()) {
            return None;
        }
        let spec = def(self.kind).shear?;
        let count = self
            .rng
            .next_range(spec.min.min(spec.max) as i32, spec.max as i32) as u8;
        let regrow = self.rng.next_range(
            spec.regrow_min.min(spec.regrow_max) as i32,
            spec.regrow_max as i32,
        ) as i64;
        self.tags_mut().insert(
            super::tags::SHEAR_REGROW.to_owned(),
            MobTagValue::Int(regrow),
        );
        Some(count)
    }
}

#[cfg(test)]
mod tests {
    use super::tick::{MobTickCtx, SpeciesMeta};
    use super::*;
    use crate::mob::brain::{AiCtx, BehaviorOutput, Brain, TickInputs};
    use crate::mob::{confined, PlayerAnchor};
    use crate::world::ServerWorld;
    use petramond_math::math::Vec3;

    mod navigation;
}
