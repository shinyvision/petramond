use std::sync::Arc;

use petramond_math::math::{IVec3, Tilt, Vec3};
use petramond_math::world_pos::WorldPos;

use super::super::anim::{AnimKind, AnimLayer};
use super::super::brain::{AttackIntent, BehaviorOutput, Brain, HeadLook};
use super::super::confined::{self, ConfinedRegion, RegionCache};
use super::super::damage::DeathState;
use super::super::kinematics::{DriveIntent, KinematicPose};
use super::super::nav::{Navigator, Unstick};
use super::super::EntityRef;

#[derive(Copy, Clone, Debug)]
#[allow(dead_code)]
pub(in crate::mob) struct Interp {
    pub pos: WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
    pub anim_time: f32,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub hurt: f32,
}

impl Interp {
    pub fn at_rest(pos: WorldPos, yaw: f32) -> Self {
        Interp {
            pos,
            yaw,
            tilt: Tilt::LEVEL,
            anim_time: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt: 0.0,
        }
    }
}

pub(in crate::mob) struct Motion {
    pub vel: Vec3,
    pub on_ground: bool,
    pub fall_peak_y: f64,
    pub fall_distance: f32,
    pub splash_drop: f32,
    pub walk_speed_scale: f32,
    pub gait_pace: f32,
    pub stepping: bool,
    pub knockback: Vec3,
    pub push: Vec3,
    pub walk_launch: bool,
    pub air_walk: bool,
    pub drive: Option<DriveIntent>,
    pub kinematic: Option<KinematicPose>,
    pub escape: petramond_world::collision::EscapeRoute,
}

impl Motion {
    pub fn at_rest(feet_y: f64) -> Self {
        Motion {
            vel: Vec3::ZERO,
            on_ground: false,
            fall_peak_y: feet_y,
            fall_distance: 0.0,
            splash_drop: 0.0,
            walk_speed_scale: 1.0,
            gait_pace: 1.0,
            stepping: false,
            knockback: Vec3::ZERO,
            push: Vec3::ZERO,
            walk_launch: false,
            air_walk: false,
            drive: None,
            kinematic: None,
            escape: Default::default(),
        }
    }
}

pub(in crate::mob) struct Combat {
    pub damage_immunity: petramond_world::damage::DamageImmunity,
    pub hurt_timer: f32,
    pub stagger_timer: f32,
    pub death: DeathState,
    pub attack: Option<AttackIntent>,
    pub attacker: Option<EntityRef>,
    pub attacker_ticks: u32,
}

impl Default for Combat {
    fn default() -> Self {
        Combat {
            damage_immunity: Default::default(),
            hurt_timer: 0.0,
            stagger_timer: 0.0,
            death: DeathState::Alive,
            attack: None,
            attacker: None,
            attacker_ticks: 0,
        }
    }
}

impl Combat {
    pub fn count_down_feedback(&mut self, dt: f32) {
        self.hurt_timer = (self.hurt_timer - dt).max(0.0);
        self.stagger_timer = (self.stagger_timer - dt).max(0.0);
    }

    pub fn age_attacker(&mut self) {
        if self.attacker.is_some() {
            self.attacker_ticks = self.attacker_ticks.saturating_add(1);
        }
    }
}

pub(in crate::mob) struct Mind {
    pub brain: Brain,
    pub nav: Navigator,
    pub unstick: Unstick,
    pub current_target: Option<EntityRef>,
    pub held_decision: HeldDecision,
    pub contacts: Vec<EntityRef>,
}

#[derive(Copy, Clone, Debug, Default)]
pub(in crate::mob) struct HeldDecision {
    head_look: Option<HeadLook>,
    facing: Option<f32>,
    speed_scale: Option<f32>,
    idle_anim: Option<u8>,
}

impl HeldDecision {
    pub fn of(decision: &BehaviorOutput) -> Self {
        HeldDecision {
            head_look: decision.head_look,
            facing: decision.facing,
            speed_scale: decision.speed_scale,
            idle_anim: decision.idle_anim,
        }
    }

    pub fn replay(self, target: Option<EntityRef>) -> BehaviorOutput {
        BehaviorOutput {
            head_look: self.head_look,
            facing: self.facing,
            speed_scale: self.speed_scale,
            idle_anim: self.idle_anim,
            target,
            ..BehaviorOutput::default()
        }
    }
}

pub(in crate::mob) struct Confinement {
    cooldown: u8,
    region: Option<Arc<ConfinedRegion>>,
    checked_at: IVec3,
    checked_rev: u64,
    free_age: u16,
}

pub(in crate::mob) struct ConfinementProbe {
    pub cell: IVec3,
    pub think: bool,
    pub judgeable: bool,
    pub nav_rev: u64,
}

impl Confinement {
    pub fn new(seed: u64) -> Self {
        Confinement {
            cooldown: ((seed % confined::CHECK_INTERVAL as u64) as u8).max(1),
            region: None,
            checked_at: IVec3::ZERO,
            checked_rev: u64::MAX,
            free_age: 0,
        }
    }

    pub fn region(&self) -> Option<&ConfinedRegion> {
        self.region.as_deref()
    }

    pub fn refresh(
        &mut self,
        probe: ConfinementProbe,
        regions: &mut RegionCache,
        flood: impl FnOnce() -> Option<ConfinedRegion>,
    ) -> Option<bool> {
        self.cooldown = self.cooldown.saturating_sub(1);
        let region_dropped = self.region.as_ref().is_some_and(|r| !regions.is_live(r));
        if region_dropped {
            self.region = None;
        }
        let verdict_stale = region_dropped
            || self.region.is_some()
            || confined::free_verdict_stale(
                probe.cell,
                self.checked_at,
                probe.nav_rev,
                self.checked_rev,
                self.free_age,
            );
        let due = probe.think && (self.cooldown == 0 || region_dropped);
        if due && !verdict_stale {
            self.cooldown = confined::CHECK_INTERVAL;
            self.free_age = self
                .free_age
                .saturating_add(confined::CHECK_INTERVAL as u16);
        }
        if !(due && verdict_stale && probe.judgeable) {
            return None;
        }
        self.checked_at = probe.cell;
        self.checked_rev = probe.nav_rev;
        self.free_age = 0;
        self.region = regions
            .region_at(probe.cell)
            .or_else(|| flood().map(|r| regions.insert(r)));
        self.cooldown = confined::CHECK_INTERVAL;
        Some(self.region.is_some())
    }
}

pub(in crate::mob) struct Presentation {
    pub active_emitters: Vec<u8>,
    pub active_anims: Vec<AnimLayer>,
    pub anim_kind: AnimKind,
    pub head_vel: [f32; 2],
    pub held: [Option<petramond_world::item::ItemType>; 2],
    pub draw: crate::world::draw::BodyDraw,
}

impl Default for Presentation {
    fn default() -> Self {
        Presentation {
            active_emitters: Vec::new(),
            active_anims: Vec::new(),
            anim_kind: AnimKind::Rest,
            head_vel: [0.0; 2],
            held: [None; 2],
            draw: Default::default(),
        }
    }
}
