use crate::player::body_claims::BodyClaims;
use crate::world::WorldData;
use petramond_math::math::{IVec3, Vec3};

const MIRRORED_CLAIM: &str = "";

pub use crate::world::session::{PlayerInputSnapshot, PlayerRosterSnapshot, UseGesture};

pub use crate::world::session::{PLAYER_HALF_W as HALF_W, PLAYER_HEIGHT as HEIGHT};
pub const EYE: f32 = 1.62;
pub const DT_MAX: f32 = 0.05;
pub const PITCH_LIMIT: f32 = 1.553_343;
pub const MAX_HEALTH: i32 = 20;

#[derive(Copy, Clone, Default)]
pub struct Input {
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
    pub sneak: bool,
}

petramond_math::wire_enum::wire_enum! {
    pub enum PlayerMode: u8 {
        Survival = 0,
        Spectator = 1,
        Creative = 2,
        CreativeFlying = 3,
    }
    default Survival
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BedSpawn {
    pub bed: IVec3,
    pub spot: IVec3,
}

#[derive(Clone)]
pub struct Player {
    pub pos: petramond_math::world_pos::WorldPos,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    mode: PlayerMode,
    pub(super) jumping: bool,
    health: i32,
    exposure: petramond_world::exposure::BodyExposure,
    damage_immunity: petramond_world::damage::DamageImmunity,
    pub(super) fall_peak_y: f64,
    fall_distance: f32,
    pub inventory: petramond_world::inventory::Inventory,
    /// Which hand the use-click ladder is currently acting from. TRANSIENT
    /// dispatch context, never saved or replicated: the ladder's second pass
    /// sets it to `Off` so every "what am I holding" read — engine consumers
    /// and the mod ABI's `PlayerState`/`PlayerHeld`/`ConsumeHeld` alike —
    /// resolves to the off-hand without the attempt payload ever naming a
    /// hand. Always reset to `Main` when the dispatch returns.
    pub acting_hand: petramond_world::inventory::Hand,
    pub(super) escape: petramond_world::collision::EscapeRoute,
    pub bed_spawn: Option<BedSpawn>,
    pub craft_craftable_only: bool,
    pub progression: super::Progression,
    effects: Vec<petramond_world::effect::ActiveEffect>,
    pub claims: BodyClaims,
    pub use_gesture: UseGesture,
}

impl Player {
    pub fn new(feet: petramond_math::world_pos::WorldPos) -> Self {
        Self {
            pos: feet,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            mode: PlayerMode::Survival,
            jumping: false,
            health: MAX_HEALTH,
            exposure: Default::default(),
            damage_immunity: Default::default(),
            fall_peak_y: feet.y,
            fall_distance: 0.0,
            inventory: petramond_world::inventory::Inventory::new(),
            acting_hand: petramond_world::inventory::Hand::Main,
            escape: Default::default(),
            bed_spawn: None,
            craft_craftable_only: false,
            progression: Default::default(),
            effects: Vec::new(),
            claims: BodyClaims::default(),
            use_gesture: UseGesture::default(),
        }
    }

    #[inline]
    pub fn held(&self) -> Option<&petramond_world::item::ItemStack> {
        self.inventory.held_in(self.acting_hand)
    }

    #[inline]
    pub fn health(&self) -> i32 {
        self.health
    }

    pub fn set_health(&mut self, health: i32) {
        self.health = health.clamp(0, MAX_HEALTH);
    }

    pub fn apply_damage(
        &mut self,
        points: i32,
        immunity: petramond_world::damage::Immunity,
    ) -> bool {
        if points <= 0
            || self.health == 0
            || self.is_invulnerable()
            || immunity.blocks(&self.damage_immunity)
        {
            return false;
        }
        self.health = (self.health - points).max(0);
        immunity.grant(&mut self.damage_immunity);
        true
    }

    #[inline]
    pub fn is_damage_immune(&self) -> bool {
        self.damage_immunity.is_active()
    }

    #[inline]
    pub fn damage_immunity(&self) -> &petramond_world::damage::DamageImmunity {
        &self.damage_immunity
    }

    #[inline]
    pub fn tick_damage_immunity(&mut self) {
        self.damage_immunity.tick();
    }

    #[inline]
    pub fn clear_damage_immunity(&mut self) {
        self.damage_immunity.clear();
    }

    pub fn entombed(&self) -> bool {
        self.escape.entombed()
    }

    pub fn conditions(&self) -> &petramond_world::condition::BodyConditions {
        self.exposure.conditions()
    }

    pub fn exposure_mut(&mut self) -> &mut petramond_world::exposure::BodyExposure {
        &mut self.exposure
    }

    pub fn clear_exposure(&mut self) {
        self.exposure.clear();
    }

    pub fn heal(&mut self, points: i32) {
        if points > 0 && self.health > 0 {
            self.health = (self.health + points).min(MAX_HEALTH);
        }
    }

    #[inline]
    pub fn effects(&self) -> &[petramond_world::effect::ActiveEffect] {
        &self.effects
    }

    pub fn apply_effect(&mut self, effect: petramond_world::effect::Effect, ticks: u32) {
        if ticks == 0 {
            self.remove_effect(effect);
            return;
        }
        match self.effects.iter_mut().find(|e| e.effect == effect) {
            Some(e) => e.remaining = ticks,
            None => self.effects.push(petramond_world::effect::ActiveEffect {
                effect,
                remaining: ticks,
            }),
        }
    }

    pub fn remove_effect(&mut self, effect: petramond_world::effect::Effect) {
        self.effects.retain(|e| e.effect != effect);
    }

    pub fn set_effects(&mut self, effects: Vec<petramond_world::effect::ActiveEffect>) {
        self.effects = effects;
    }

    fn effect_speed_scale(&self) -> f32 {
        self.effects
            .iter()
            .map(|e| e.effect.def().behavior.speed_scale())
            .product()
    }

    pub fn refresh_engine_claims(&mut self, gameplay: bool) {
        self.claims.set_attribute(
            super::ENGINE_CLAIMANT,
            mod_api::PlayerAttribute::MoveSpeed,
            self.effect_speed_scale(),
        );
        let idle = self.is_spectator() || !gameplay;
        let denied = if idle {
            super::DeniedActions::of([
                mod_api::BodyAction::Attack,
                mod_api::BodyAction::Mine,
                mod_api::BodyAction::Use,
            ])
        } else {
            super::DeniedActions::NONE
        };
        self.claims
            .set_denied_actions(super::ENGINE_CLAIMANT, denied);
    }

    #[inline]
    pub fn move_scale(&self) -> f32 {
        self.claims.attribute(mod_api::PlayerAttribute::MoveSpeed)
    }

    #[inline]
    pub fn fly_scale(&self) -> f32 {
        self.claims.attribute(mod_api::PlayerAttribute::FlySpeed)
    }

    #[inline]
    pub fn scaled_ticks(&self, attribute: mod_api::PlayerAttribute, base: u32) -> u32 {
        (base as f32 * self.claims.attribute(attribute)).round() as u32
    }

    #[inline]
    pub fn denied_actions(&self) -> super::DeniedActions {
        self.claims.denied_actions()
    }

    pub fn adopt_resolved_body(
        &mut self,
        scale: f32,
        fly_scale: f32,
        denied: super::DeniedActions,
    ) {
        self.claims
            .set_attribute(MIRRORED_CLAIM, mod_api::PlayerAttribute::MoveSpeed, scale);
        self.claims.set_attribute(
            MIRRORED_CLAIM,
            mod_api::PlayerAttribute::FlySpeed,
            fly_scale,
        );
        self.claims.set_denied_actions(MIRRORED_CLAIM, denied);
    }

    pub fn clear_effects(&mut self) {
        self.effects.clear();
    }

    pub fn tick_effects(&mut self) -> Vec<petramond_world::effect::EffectBehavior> {
        let mut fired = Vec::new();
        for e in &mut self.effects {
            e.remaining -= 1;
            let behavior = e.effect.def().behavior;
            match behavior {
                petramond_world::effect::EffectBehavior::None
                | petramond_world::effect::EffectBehavior::Speed { .. } => {}
                petramond_world::effect::EffectBehavior::Regen { interval, .. } => {
                    if e.remaining % interval == 0 {
                        fired.push(behavior);
                    }
                }
            }
        }
        self.effects.retain(|e| e.remaining > 0);
        fired
    }

    pub fn apply_knockback(&mut self, impulse: Vec3) {
        if self.is_invulnerable() {
            return;
        }
        self.vel += impulse;
        if impulse.y > 0.0 {
            self.on_ground = false;
            self.jumping = false;
        }
    }

    #[cfg(test)]
    pub fn take_fall_distance(&mut self) -> f32 {
        std::mem::replace(&mut self.fall_distance, 0.0)
    }

    pub fn teleport(&mut self, pos: petramond_math::world_pos::WorldPos) {
        self.pos = pos;
        self.fall_peak_y = pos.y;
        self.fall_distance = 0.0;
    }

    pub(super) fn track_fall(&mut self, was_on_ground: bool, controlled: bool) {
        if controlled {
            self.fall_peak_y = self.pos.y;
        } else if self.on_ground {
            if !was_on_ground {
                let dist = (self.fall_peak_y - self.pos.y) as f32;
                if dist > self.fall_distance {
                    self.fall_distance = dist;
                }
            }
            self.fall_peak_y = self.pos.y;
        } else {
            self.fall_peak_y = self.fall_peak_y.max(self.pos.y);
        }
    }

    pub fn rotate(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    #[inline]
    pub fn mode(&self) -> PlayerMode {
        self.mode
    }

    #[inline]
    pub fn is_spectator(&self) -> bool {
        self.mode == PlayerMode::Spectator
    }

    pub fn is_creative(&self) -> bool {
        matches!(self.mode, PlayerMode::Creative | PlayerMode::CreativeFlying)
    }

    pub fn is_flying(&self) -> bool {
        self.abilities().flying
    }

    pub fn is_invulnerable(&self) -> bool {
        self.abilities().invulnerable
    }

    pub fn set_mode(&mut self, mode: PlayerMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.vel = Vec3::ZERO;
        self.on_ground = false;
        self.jumping = false;
        self.fall_peak_y = self.pos.y;
        self.fall_distance = 0.0;
    }

    pub fn toggle_mode(&mut self) {
        let next = match self.mode {
            PlayerMode::Survival | PlayerMode::Creative | PlayerMode::CreativeFlying => {
                PlayerMode::Spectator
            }
            PlayerMode::Spectator => PlayerMode::Survival,
        };
        self.set_mode(next);
    }

    #[inline]
    pub fn eye(&self) -> petramond_math::world_pos::WorldPos {
        self.pos + Vec3::new(0.0, EYE, 0.0)
    }

    #[inline]
    pub fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(self.yaw.sin() * cp, self.pitch.sin(), self.yaw.cos() * cp).normalize()
    }

    #[inline]
    pub fn body_center(&self) -> petramond_math::world_pos::WorldPos {
        self.pos + Vec3::new(0.0, HEIGHT * 0.5, 0.0)
    }

    #[inline]
    pub fn body(&self) -> petramond_world::body::Body {
        petramond_world::body::Body::new(self.pos, HALF_W, HEIGHT)
    }

    #[inline]
    pub(super) fn aabb_min(&self) -> [f64; 3] {
        let hw = f64::from(HALF_W);
        [self.pos.x - hw, self.pos.y, self.pos.z - hw]
    }

    #[inline]
    pub(super) fn aabb_max(&self) -> [f64; 3] {
        let hw = f64::from(HALF_W);
        [
            self.pos.x + hw,
            self.pos.y + f64::from(HEIGHT),
            self.pos.z + hw,
        ]
    }

    pub fn columns_loaded(&self, world: &WorldData) -> bool {
        let hw = f64::from(HALF_W);
        let cx0 = (self.pos.x - hw).floor() as i32 >> 4;
        let cx1 = (self.pos.x + hw).floor() as i32 >> 4;
        let cz0 = (self.pos.z - hw).floor() as i32 >> 4;
        let cz1 = (self.pos.z + hw).floor() as i32 >> 4;
        for cx in cx0..=cx1 {
            for cz in cz0..=cz1 {
                if !world.chunk_loaded(cx, cz) {
                    return false;
                }
            }
        }
        true
    }
}
