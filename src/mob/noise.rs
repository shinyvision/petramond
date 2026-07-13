//! Gameplay NOISE — the perception seam hearing-based mob AI consumes.
//!
//! A [`Noise`] is one tick-scoped record of an audible gameplay action: a player
//! or mob footstep, a block placed, a block broken. Emitters push records into the
//! world's noise sink ([`World::push_noise`](crate::world::World::push_noise));
//! the mob manager hands the accumulated batch to every mob's AI tick as
//! `AiCtx::noises`, then clears it. Nothing here decides who *reacts* — hearing
//! radii, memory, and target policy live on the listening brain nodes
//! (`chase_sound`), so what a species hears is row data, not an engine rule.
//!
//! Timing contract: player and block noises emitted during a tick's earlier
//! stages are heard by the mob stage of the SAME tick; a mob's own footsteps are
//! recorded while the mobs tick and are heard on the NEXT tick (the batch a mob
//! tick reads is snapshotted before any mob moves, so hearing is independent of
//! mob iteration order — determinism over freshness).
//!
//! Loudness is deliberately NOT emitter data yet: every record carries its
//! [`NoiseKind`], and listeners apply their own radius. If a future listener
//! needs kind-dependent ranges, put the tuning on ITS node params — the
//! vocabulary here already distinguishes the kinds.

use crate::mathh::Vec3;

use super::EntityRef;

/// The audible action a [`Noise`] records.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NoiseKind {
    /// A footstep: a player moving un-sneakily, or a walking mob.
    Step,
    /// A block placed by a player.
    BlockPlaced,
    /// A block broken by a player. Sim-destroyed blocks (natural breaks) are
    /// deliberately silent: they carry no actor a listener could lock onto.
    BlockBroken,
}

/// One audible gameplay action, tick-scoped. `source` is the entity a hearing
/// listener may lock onto; `pos` is where the sound happened (an actor's feet,
/// a block's centre) — for a block action that is NOT the actor's position.
#[derive(Copy, Clone, Debug)]
pub struct Noise {
    pub pos: Vec3,
    pub kind: NoiseKind,
    pub source: EntityRef,
}

/// Minimum horizontal speed (m/s) at which a player's movement is audible.
/// Sits between sneak speed (2.15) and walk speed (4.3), so walking and
/// sprinting step audibly while drift, jostling, and water currents stay
/// quiet. Sneaking is silent by the flag, not this threshold — the threshold
/// only filters non-locomotion movement.
pub const STEP_NOISE_MIN_SPEED: f32 = 3.0;

/// Whether a player moving at `horizontal_speed_sq` (m/s, squared) makes step
/// noise this tick. Airborne players are silent (a jump's arc is a quiet
/// window; landings resume stepping on the first grounded tick).
pub fn player_steps_are_audible(
    horizontal_speed_sq: f32,
    on_ground: bool,
    sneaking: bool,
    spectator: bool,
) -> bool {
    !spectator
        && !sneaking
        && on_ground
        && horizontal_speed_sq >= STEP_NOISE_MIN_SPEED * STEP_NOISE_MIN_SPEED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_audible_only_for_grounded_unsneaky_locomotion() {
        let walk_sq = 4.3f32 * 4.3;
        let sneak_sq = 2.15f32 * 2.15;
        assert!(player_steps_are_audible(walk_sq, true, false, false));
        assert!(
            !player_steps_are_audible(walk_sq, true, true, false),
            "sneaking is silent at any speed"
        );
        assert!(
            !player_steps_are_audible(walk_sq, false, false, false),
            "airborne movement is silent"
        );
        assert!(
            !player_steps_are_audible(walk_sq, true, false, true),
            "spectators have no feet"
        );
        assert!(
            !player_steps_are_audible(sneak_sq, true, false, false),
            "sub-threshold drift is silent"
        );
    }
}
