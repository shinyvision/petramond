use std::collections::BTreeMap;
use std::sync::Arc;

use petramond_math::math::{IVec3, Tilt};

pub use super::anim::AnimLayer;
use super::nav::Navigator;
use super::{def, EntityRef, Mob, MobRng, MobTagValue, DEFAULT_DAMAGE_FLASH_SECS};

mod parts;
pub(super) mod tick;

use parts::{Combat, Confinement, Interp, Mind, Motion, Presentation};
pub(super) use tick::{Begun, Footing, MobTickCtx, MotionStart, SpeciesMeta};

const HURT_FLASH_SECS: f32 = DEFAULT_DAMAGE_FLASH_SECS;

pub fn hurt_flash01(prev: f32, curr: f32, alpha: f32) -> f32 {
    let t = prev + (curr - prev) * alpha;
    (t / HURT_FLASH_SECS).clamp(0.0, 1.0)
}

pub struct Instance {
    pub(super) id: u64,
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    pub(super) interp: Interp,
    pub(super) motion: Motion,
    pub(super) combat: Combat,
    pub(super) mind: Mind,
    confinement: Confinement,
    pub(super) presentation: Presentation,
    tags: Arc<BTreeMap<String, MobTagValue>>,
    /// Bumped on every tag write: what lets a reader tell an unchanged tag map apart cheaply.
    tags_rev: u64,
    exposure: petramond_world::exposure::BodyExposure,
    pub(super) distance_despawned: bool,
    pub(super) rng: MobRng,
    container: petramond_world::container::Container,
    dig: DrivenDig,
}

#[derive(Clone, Debug, Default)]
pub struct DrivenDig {
    mining: petramond_world::mining::MiningState,
    tick: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DigStep {
    Digging(f32),
    Done(petramond_world::mining::BreakEvent),
}

impl Instance {
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
            tags_rev: 0,
            exposure: petramond_world::exposure::BodyExposure::new(d.tolerates),
            distance_despawned: false,
            rng: MobRng::new(seed),
            container: petramond_world::container::Container::with_len(d.container_slots),
            dig: DrivenDig::default(),
        }
    }

    #[inline]
    pub fn exposure(&self) -> &petramond_world::exposure::BodyExposure {
        &self.exposure
    }

    #[inline]
    pub fn exposure_mut(&mut self) -> &mut petramond_world::exposure::BodyExposure {
        &mut self.exposure
    }

    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub(super) fn nav_search_waiting(&self) -> bool {
        self.mind.nav.search_waiting()
    }

    #[inline]
    pub(super) fn take_attack(&mut self) -> Option<super::brain::AttackIntent> {
        self.combat.attack.take()
    }

    #[inline]
    pub fn active_emitters(&self) -> &[u8] {
        &self.presentation.active_emitters
    }

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

    pub fn aabb(&self) -> ([f64; 3], [f64; 3]) {
        self.body().aabb()
    }

    pub(super) fn body(&self) -> petramond_world::body::Body {
        let s = def(self.kind).size;
        petramond_world::body::Body::new(self.pos, s.half_width, s.height)
    }

    pub(super) fn set_contacts(&mut self, contacts: impl IntoIterator<Item = EntityRef>) {
        self.mind.contacts.clear();
        self.mind.contacts.extend(contacts);
    }

    #[inline]
    pub fn contacts(&self) -> &[EntityRef] {
        &self.mind.contacts
    }

    #[inline]
    pub fn is_shorn(&self) -> bool {
        self.tag_int(super::tags::SHEAR_REGROW) > 0
    }

    #[inline]
    fn tag_int(&self, key: &str) -> i64 {
        self.tags
            .get(key)
            .and_then(MobTagValue::as_int)
            .unwrap_or(0)
    }

    #[inline]
    pub fn tags(&self) -> &BTreeMap<String, MobTagValue> {
        &self.tags
    }

    #[inline]
    pub fn container(&self) -> &petramond_world::container::Container {
        &self.container
    }

    #[inline]
    pub fn container_mut(&mut self) -> &mut petramond_world::container::Container {
        &mut self.container
    }

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

    pub fn dig_overlay(&self, now: u64) -> Option<(IVec3, u8)> {
        (self.dig.tick + 1 >= now)
            .then(|| self.dig.mining.overlay())
            .flatten()
    }

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

    pub fn restore_container(&mut self, mut saved: petramond_world::container::Container) {
        let declared = def(self.kind).container_slots;
        if saved.slots.len() < declared {
            saved.slots.resize(declared, None);
        }
        self.container = saved;
    }

    pub fn take_container_items(&mut self) -> Vec<petramond_world::item::ItemStack> {
        self.container
            .slots
            .iter_mut()
            .filter_map(Option::take)
            .collect()
    }

    #[inline]
    pub(super) fn tags_shared(&self) -> Arc<BTreeMap<String, MobTagValue>> {
        Arc::clone(&self.tags)
    }

    #[inline]
    pub(super) fn tags_mut(&mut self) -> &mut BTreeMap<String, MobTagValue> {
        self.tags_rev = self.tags_rev.wrapping_add(1);
        Arc::make_mut(&mut self.tags)
    }

    /// The tag map's revision: unchanged while no tag was written.
    #[inline]
    pub fn tags_rev(&self) -> u64 {
        self.tags_rev
    }

    #[inline]
    pub(in crate::mob) fn confinement_mut(&mut self) -> &mut Confinement {
        &mut self.confinement
    }

    pub fn is_confined(&self) -> bool {
        self.tags
            .get(super::tags::CONFINED)
            .and_then(MobTagValue::as_bool)
            == Some(true)
    }

    pub(super) fn overlay_tags(&mut self, saved: BTreeMap<String, MobTagValue>) {
        let tags = self.tags_mut();
        for (k, v) in saved {
            tags.insert(k, v);
        }
    }

    pub(super) fn shear(&mut self) -> Option<u8> {
        if !crate::rules::item_use::can_shear_coat(
            self.kind,
            self.combat.death.is_dead(),
            self.is_shorn(),
        ) {
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
