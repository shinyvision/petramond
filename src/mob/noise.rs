//! Gameplay NOISE — the perception seam hearing-based mob AI consumes.
//!
//! A [`Noise`] is one tick-scoped record of an audible gameplay action: a player
//! or mob footstep, a block placed, a block broken. Emitters push records into the
//! world's noise sink ([`World::push_noise`](crate::world::ServerWorld::push_noise));
//! the mob manager hands the accumulated batch to every mob's AI tick as
//! `AiCtx::noises`, then clears it. Nothing here decides who *reacts* — hearing
//! radii, memory, and target policy live on the listening brain nodes
//! (`chase_sound`), so what a species hears is row data, not an engine rule.
//!
//! The heard batch is a [`NoiseField`]: bucketed by horizontal column once
//! per tick, so a listener visits only the noises around it — hearing costs
//! scale with the noise near each listener, not with every noise in the world.
//! Only species that declare `"step_noise"` (the default) record footsteps at
//! all; a silent body (a boat, a cart) never enters the batch.
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

use petramond_math::world_pos::WorldPos;

use super::spatial::ColumnIndex;
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
    pub pos: WorldPos,
    pub kind: NoiseKind,
    pub source: EntityRef,
}

/// One tick's heard noise batch, bucketed by horizontal column (see the
/// module docs). Built by the mob manager from everything pushed since the
/// last mob tick; read by every listener through [`near`](Self::near).
#[derive(Default)]
pub struct NoiseField {
    noises: Vec<Noise>,
    grid: ColumnIndex,
}

impl NoiseField {
    /// A field holding exactly `noises`, in that order — fixtures and one-off
    /// callers; the manager reuses one field through [`take_batch`](Self::take_batch).
    pub fn from_noises(noises: impl IntoIterator<Item = Noise>) -> Self {
        let mut field = NoiseField::default();
        field.noises.extend(noises);
        field.reindex();
        field
    }

    /// The shared empty field, for contexts that hear nothing.
    pub fn empty() -> &'static NoiseField {
        static EMPTY: std::sync::LazyLock<NoiseField> =
            std::sync::LazyLock::new(NoiseField::default);
        &EMPTY
    }

    /// Make `pending` this field's batch and re-bucket it. The buffers swap,
    /// so `pending` comes back empty with the previous batch's capacity.
    pub fn take_batch(&mut self, pending: &mut Vec<Noise>) {
        std::mem::swap(&mut self.noises, pending);
        pending.clear();
        self.reindex();
    }

    /// Drop the batch unheard.
    pub fn clear(&mut self) {
        self.noises.clear();
        self.grid.rebuild(std::iter::empty());
    }

    pub fn len(&self) -> usize {
        self.noises.len()
    }

    pub fn is_empty(&self) -> bool {
        self.noises.is_empty()
    }

    fn reindex(&mut self) {
        self.grid.rebuild(
            self.noises
                .iter()
                .enumerate()
                .map(|(i, n)| (n.pos, i as u32)),
        );
    }

    /// Every noise within 3-D distance `radius` of `pos`, with its index in
    /// the batch (the order it was pushed in — ties between equally distant
    /// noises break on it, so a listener's pick never depends on the grid).
    pub fn near(&self, pos: WorldPos, radius: f32) -> impl Iterator<Item = (usize, &Noise)> + '_ {
        let r2 = radius * radius;
        self.grid
            .candidates(pos, f64::from(radius.max(0.0)))
            .map(|i| (i as usize, &self.noises[i as usize]))
            .filter(move |(_, n)| (n.pos - pos).length_squared() <= r2)
    }
}

/// Minimum horizontal speed (m/s) at which a player's movement is audible.
/// Sits between sneak speed (2.15) and walk speed (4.3), so walking and
/// sprinting step audibly while drift, jostling, and fluid currents stay
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
    use crate::player::PlayerId;

    fn step(x: f64, z: f64, player: u8) -> Noise {
        Noise {
            pos: WorldPos::new(x, 64.0, z),
            kind: NoiseKind::Step,
            source: EntityRef::Player(PlayerId(player)),
        }
    }

    #[test]
    fn near_matches_a_brute_force_radius_scan() {
        let mut rng = crate::mob::MobRng::new(11);
        let noises: Vec<Noise> = (0..300)
            .map(|i| {
                let x = f64::from(rng.next_f32()) * 160.0 - 80.0;
                let z = f64::from(rng.next_f32()) * 160.0 - 80.0;
                step(x, z, (i % 200) as u8)
            })
            .collect();
        let field = NoiseField::from_noises(noises.iter().copied());
        for (x, z, radius) in [(0.0, 0.0, 12.0), (-15.9, 16.1, 5.0), (40.0, -40.0, 30.0)] {
            let pos = WorldPos::new(x, 64.0, z);
            let mut got: Vec<usize> = field.near(pos, radius).map(|(i, _)| i).collect();
            got.sort_unstable();
            let want: Vec<usize> = noises
                .iter()
                .enumerate()
                .filter(|(_, n)| (n.pos - pos).length_squared() <= radius * radius)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(got, want, "probe at ({x}, {z}) r {radius}");
        }
    }

    #[test]
    fn take_batch_swaps_buffers_and_empties_the_pending_side() {
        let mut field = NoiseField::from_noises([step(0.0, 0.0, 1)]);
        let mut pending = vec![step(3.0, 0.0, 2), step(100.0, 0.0, 3)];
        field.take_batch(&mut pending);
        assert!(pending.is_empty());
        assert_eq!(field.len(), 2);
        let heard: Vec<EntityRef> = field
            .near(WorldPos::new(0.0, 64.0, 0.0), 10.0)
            .map(|(_, n)| n.source)
            .collect();
        assert_eq!(heard, vec![EntityRef::Player(PlayerId(2))]);
        field.clear();
        assert!(field.is_empty());
        assert_eq!(field.near(WorldPos::new(3.0, 64.0, 0.0), 10.0).count(), 0);
    }

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
