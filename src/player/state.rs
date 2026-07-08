use crate::mathh::{IVec3, Vec3};
use crate::world::World;

/// Half the horizontal width (box is 0.6 wide on x and z).
pub const HALF_W: f32 = 0.3;
/// Full body height.
pub const HEIGHT: f32 = 1.8;
/// Eye height above the feet (1.62).
pub const EYE: f32 = 1.62;
/// Largest physics sub-step; `app` splits a frame's `dt` into chunks this size
/// so a long stall can't make one update step move (and tunnel) too far.
pub const DT_MAX: f32 = 0.05;
/// Pitch is clamped to just shy of straight up/down (~89°) so the look never
/// tips through vertical — past it the view flips and yaw inverts (gimbal).
pub const PITCH_LIMIT: f32 = 1.553_343;
/// Full health, in half-heart points: 20 points = 10 hearts. Health is integer
/// half-hearts so the HUD renders full/half/empty cells directly from it.
pub const MAX_HEALTH: i32 = 20;

/// Per-frame movement intent, in world space.
#[derive(Copy, Clone, Default)]
pub struct Input {
    /// Wish direction (unit length, or zero). Survival uses the horizontal XZ
    /// components; spectator mode uses the full 3-D vector.
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlayerMode {
    Survival,
    Spectator,
}

/// The player's bed spawn point: which bed owns it (the bed's rotated-footprint
/// base cell — cleared when that bed is destroyed) and the safe standing cell
/// chosen beside it when the spawn was set (the respawn target when the bed's
/// area isn't loaded for a fresh scan). Set by interacting with a bed
/// (`game::bed`), persisted in `level.dat`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BedSpawn {
    pub bed: IVec3,
    pub spot: IVec3,
}

#[derive(Clone)]
pub struct Player {
    /// Feet centre (see module docs).
    pub pos: Vec3,
    pub vel: Vec3,
    /// Look direction, radians. `yaw` turns about +Y; `pitch` tilts up/down,
    /// clamped to [`PITCH_LIMIT`]. The player is the authority for the facing —
    /// the camera mirrors these onto its own orientation each frame, exactly as
    /// `cam.pos` mirrors [`eye`](Self::eye) — so the look persists in `level.dat`
    /// alongside the rest of the player state.
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    mode: PlayerMode,
    /// True between a jump take-off and the next blocked vertical sweep (landing
    /// or head-bonk). Gates the apex easing so only a genuine jump arc is
    /// softened — walking off a ledge or bonking a ceiling falls at full gravity.
    pub(super) jumping: bool,
    /// Current health in half-heart points (`0..=`[`MAX_HEALTH`]). Fall damage is the
    /// only source for now; it is mutated on the deterministic tick (see
    /// `crate::game::health`), never in per-frame physics — the physics only *measures*
    /// a fall (below).
    health: i32,
    /// Highest feet-`y` reached since the player last stood on the ground (or was in
    /// water). The fall distance of a landing is this minus the landing `y`. Reset when
    /// grounded/submerged so a fall is measured from where it began, and the arc of a
    /// jump counts from its apex, not its take-off.
    pub(super) fall_peak_y: f32,
    /// Fall distance (blocks) of the hardest landing since the tick last consumed it —
    /// latched by per-frame physics, drained on the tick where it becomes damage. Kept
    /// as the max (not a sum) because two damaging landings can't occur within one 50 ms
    /// tick, and it keeps the physics side free of the damage rule.
    fall_distance: f32,
    /// The player's 36-slot inventory (9 hotbar + 27 main). Owns the active
    /// hotbar selection that drives the held item and placement.
    pub inventory: crate::inventory::Inventory,
    /// Bed spawn point, if a bed interaction set one (see [`BedSpawn`]).
    pub bed_spawn: Option<BedSpawn>,
    /// Active status effects, in application order (deterministic iteration).
    /// Stepped once per game tick by `Game::tick_effects`; persisted by
    /// registry name in `level.dat`.
    effects: Vec<crate::effect::ActiveEffect>,
}

impl Player {
    pub fn new(feet: Vec3) -> Self {
        Self {
            pos: feet,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            mode: PlayerMode::Survival,
            jumping: false,
            health: MAX_HEALTH,
            fall_peak_y: feet.y,
            fall_distance: 0.0,
            inventory: crate::inventory::Inventory::new(),
            bed_spawn: None,
            effects: Vec::new(),
        }
    }

    /// Current health in half-heart points (`0..=`[`MAX_HEALTH`]).
    #[inline]
    pub fn health(&self) -> i32 {
        self.health
    }

    /// Overwrite health (clamped to `0..=`[`MAX_HEALTH`]). Used to restore a saved
    /// player; gameplay damage goes through [`apply_damage`](Self::apply_damage).
    pub fn set_health(&mut self, health: i32) {
        self.health = health.clamp(0, MAX_HEALTH);
    }

    /// Subtract `points` half-hearts of damage, never below zero. A no-op for a
    /// non-positive amount. Call this on the tick, not in per-frame physics.
    pub fn apply_damage(&mut self, points: i32) {
        if points > 0 {
            self.health = (self.health - points).max(0);
        }
    }

    /// Add `points` half-hearts, capped at [`MAX_HEALTH`]. A no-op for a
    /// non-positive amount AND for a dead player (0 health) — healing never
    /// resurrects; respawn owns that transition. Call on the tick.
    pub fn heal(&mut self, points: i32) {
        if points > 0 && self.health > 0 {
            self.health = (self.health + points).min(MAX_HEALTH);
        }
    }

    /// Active status effects in application order.
    #[inline]
    pub fn effects(&self) -> &[crate::effect::ActiveEffect] {
        &self.effects
    }

    /// Grant `effect` for `ticks`. An already-active effect is overwritten with
    /// the new duration (keeping its original slot in the application order);
    /// zero ticks removes it. Call on the tick.
    pub fn apply_effect(&mut self, effect: crate::effect::Effect, ticks: u32) {
        if ticks == 0 {
            self.remove_effect(effect);
            return;
        }
        match self.effects.iter_mut().find(|e| e.effect == effect) {
            Some(e) => e.remaining = ticks,
            None => self.effects.push(crate::effect::ActiveEffect {
                effect,
                remaining: ticks,
            }),
        }
    }

    /// Remove `effect` if active.
    pub fn remove_effect(&mut self, effect: crate::effect::Effect) {
        self.effects.retain(|e| e.effect != effect);
    }

    /// Clear every active effect (death/respawn starts a fresh life).
    pub fn clear_effects(&mut self) {
        self.effects.clear();
    }

    /// Step every active effect one game tick: count down, drop expired
    /// entries, and return each behavior whose interval boundary fired this
    /// tick (boundaries land every `interval` ticks counted back from expiry,
    /// including one AT expiry). The player owns WHEN a behavior fires — a
    /// duration concern — but never WHAT it does: consequences are applied by
    /// `Game::tick_effects`, because damaging behaviors must route through
    /// the `Game::damage_player` funnel this type cannot reach.
    pub(crate) fn tick_effects(&mut self) -> Vec<crate::effect::EffectBehavior> {
        let mut fired = Vec::new();
        for e in &mut self.effects {
            e.remaining -= 1;
            let behavior = e.effect.def().behavior;
            match behavior {
                crate::effect::EffectBehavior::None => {}
                crate::effect::EffectBehavior::Regen { interval, .. } => {
                    if e.remaining % interval == 0 {
                        fired.push(behavior);
                    }
                }
            }
        }
        self.effects.retain(|e| e.remaining > 0);
        fired
    }

    /// Add a knockback impulse to the velocity — a mob strike's shove (and, later,
    /// a mod HostCall). Call on the tick; the per-frame physics then integrates it
    /// like any other velocity (friction bleeds the horizontal part, gravity the
    /// vertical). Spectators float free of the world and take none — mirroring how
    /// they take no damage.
    pub fn apply_knockback(&mut self, impulse: Vec3) {
        if self.is_spectator() {
            return;
        }
        self.vel += impulse;
        // An upward pop must read as a launch, not be swallowed by the grounded
        // state (mirrors a mob's knockback clearing its own on_ground).
        if impulse.y > 0.0 {
            self.on_ground = false;
            self.jumping = false;
        }
    }

    /// Take and clear the pending fall distance (blocks) latched by the last landing.
    /// Physics still measures falls per frame (for the `track_fall` tests), but since
    /// multiplayer Phase C2b the server converts its own replicated-transform fall
    /// tracking into damage instead (`ConnectedPlayer::fall`) — nothing consumes this
    /// latch in the game anymore.
    #[cfg(test)]
    pub(crate) fn take_fall_distance(&mut self) -> f32 {
        std::mem::replace(&mut self.fall_distance, 0.0)
    }

    /// Move the feet to `pos` (a mod `Teleport` HostCall), clearing the fall
    /// bookkeeping — re-anchoring the peak and dropping any pending landing —
    /// so a teleport can never be measured as a fall. Velocity is kept.
    pub fn teleport(&mut self, pos: Vec3) {
        self.pos = pos;
        self.fall_peak_y = pos.y;
        self.fall_distance = 0.0;
    }

    /// Update the fall bookkeeping after a physics sub-step has resolved `on_ground`
    /// and the final feet `y`. `in_water` cancels the fall (water breaks a fall), a
    /// fresh landing (`was_on_ground` false, now grounded) latches its distance, and
    /// while airborne the peak tracks the highest point of the arc.
    pub(super) fn track_fall(&mut self, was_on_ground: bool, in_water: bool) {
        if in_water {
            self.fall_peak_y = self.pos.y;
        } else if self.on_ground {
            if !was_on_ground {
                let dist = self.fall_peak_y - self.pos.y;
                if dist > self.fall_distance {
                    self.fall_distance = dist;
                }
            }
            self.fall_peak_y = self.pos.y;
        } else {
            self.fall_peak_y = self.fall_peak_y.max(self.pos.y);
        }
    }

    /// Turn the look by `(dyaw, dpitch)` radians (mouse delta × sensitivity).
    /// Yaw wraps freely; pitch is clamped to [`PITCH_LIMIT`] so the view can't
    /// tip past vertical.
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

    pub fn set_mode(&mut self, mode: PlayerMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.vel = Vec3::ZERO;
        self.on_ground = false;
        self.jumping = false;
        // A mode switch is not a fall: re-anchor the peak and drop any pending landing so
        // dropping out of spectator (or into it) never lands as fall damage.
        self.fall_peak_y = self.pos.y;
        self.fall_distance = 0.0;
    }

    pub fn toggle_mode(&mut self) {
        let next = match self.mode {
            PlayerMode::Survival => PlayerMode::Spectator,
            PlayerMode::Spectator => PlayerMode::Survival,
        };
        self.set_mode(next);
    }

    /// Eye position (camera origin).
    #[inline]
    pub fn eye(&self) -> Vec3 {
        Vec3::new(self.pos.x, self.pos.y + EYE, self.pos.z)
    }

    /// View direction from yaw/pitch — the sim-side twin of
    /// [`crate::camera::Camera::forward`]. Per-player actions (placement
    /// facing, bucket rays, thrown drops) read THIS, not the camera: the
    /// camera is presentation, exists only for the local player, and can lag
    /// the eye during a step-up glide.
    #[inline]
    pub fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(self.yaw.sin() * cp, self.pitch.sin(), self.yaw.cos() * cp).normalize()
    }

    /// Centre of the body AABB (feet + half height). Used as the pickup-radius
    /// centre so a drop resting at the player's feet is measured from the body,
    /// not the eye (contract §6: "within pickup radius of player AABB").
    #[inline]
    pub fn body_center(&self) -> Vec3 {
        Vec3::new(self.pos.x, self.pos.y + HEIGHT * 0.5, self.pos.z)
    }

    /// AABB min corner.
    #[inline]
    pub(super) fn aabb_min(&self) -> Vec3 {
        Vec3::new(self.pos.x - HALF_W, self.pos.y, self.pos.z - HALF_W)
    }

    /// AABB max corner.
    #[inline]
    pub(super) fn aabb_max(&self) -> Vec3 {
        Vec3::new(
            self.pos.x + HALF_W,
            self.pos.y + HEIGHT,
            self.pos.z + HALF_W,
        )
    }

    /// True if every chunk column the horizontal AABB overlaps is loaded. The
    /// caller gates physics on this (once per frame) so the player can't fall
    /// through terrain that hasn't generated yet (spawn, or running past the
    /// load frontier). Column membership can't change within a frame, so this
    /// need not be re-checked per sub-step.
    pub fn columns_loaded(&self, world: &World) -> bool {
        let cx0 = (self.pos.x - HALF_W).floor() as i32 >> 4;
        let cx1 = (self.pos.x + HALF_W).floor() as i32 >> 4;
        let cz0 = (self.pos.z - HALF_W).floor() as i32 >> 4;
        let cz1 = (self.pos.z + HALF_W).floor() as i32 >> 4;
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
