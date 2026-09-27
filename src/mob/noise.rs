use petramond_math::world_pos::WorldPos;

use super::spatial::ColumnIndex;
use super::EntityRef;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NoiseKind {
    Step,
    BlockPlaced,
    BlockBroken,
}

#[derive(Copy, Clone, Debug)]
pub struct Noise {
    pub pos: WorldPos,
    pub kind: NoiseKind,
    pub source: EntityRef,
}

#[derive(Default)]
pub struct NoiseField {
    noises: Vec<Noise>,
    grid: ColumnIndex,
}

impl NoiseField {
    pub fn from_noises(noises: impl IntoIterator<Item = Noise>) -> Self {
        let mut field = NoiseField::default();
        field.noises.extend(noises);
        field.reindex();
        field
    }

    pub fn empty() -> &'static NoiseField {
        static EMPTY: std::sync::LazyLock<NoiseField> =
            std::sync::LazyLock::new(NoiseField::default);
        &EMPTY
    }

    pub fn take_batch(&mut self, pending: &mut Vec<Noise>) {
        std::mem::swap(&mut self.noises, pending);
        pending.clear();
        self.reindex();
    }

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

    pub fn near(&self, pos: WorldPos, radius: f32) -> impl Iterator<Item = (usize, &Noise)> + '_ {
        let r2 = radius * radius;
        self.grid
            .candidates(pos, f64::from(radius.max(0.0)))
            .map(|i| (i as usize, &self.noises[i as usize]))
            .filter(move |(_, n)| (n.pos - pos).length_squared() <= r2)
    }
}

pub const STEP_NOISE_MIN_SPEED: f32 = 3.0;

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
