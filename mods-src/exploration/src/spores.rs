//! Biome-wide spore haze, client side.
//!
//! The flora emitters show where spores come from, as rare puffs off a sporeshroom or vine. This
//! layer is the air itself, a volume that follows the camera everywhere in the biome, far from a
//! mushroom as much as under one. It's lit by the world, so it's bright under a glowcap and gone in
//! a pocket nothing lights.
//!
//! The engine derives the volume per frame from the bundle row. We only pick how much of it to show
//! and which way the air moves.

use mod_sdk::*;

use crate::keys;

const SAMPLE_INTERVAL: u64 = 15;

const PROBE_RADIUS: i32 = 16;

const SMOOTH_TAU: f32 = 1.5;

const EDDY_SPEED: f32 = 0.12;
const EDDY_PERIOD: f32 = 97.0;
const EDDY_PERIOD_2: f32 = 41.0;

const RING: [[i32; 2]; 8] = [
    [PROBE_RADIUS, 0],
    [-PROBE_RADIUS, 0],
    [0, PROBE_RADIUS],
    [0, -PROBE_RADIUS],
    [11, 11],
    [11, -11],
    [-11, 11],
    [-11, -11],
];

#[derive(Default)]
pub(crate) struct Spores {
    biome: Option<u8>,
    frame: u64,
    clock: f32,
    intensity: f32,
}

impl Spores {
    pub(crate) fn init(&mut self) {
        self.biome = resolve_underground_biome(keys::MUSHROOM_CAVERN);
        if self.biome.is_none() {
            log("exploration: no mushroom-cavern biome row; the spore haze stays off");
        }
    }

    pub(crate) fn frame(&mut self, frame: &ClientFrameData) {
        let Some(biome) = self.biome else {
            return;
        };
        self.frame = self.frame.wrapping_add(1);
        self.clock += frame.dt.clamp(0.0, 0.25);
        if !self.frame.is_multiple_of(SAMPLE_INTERVAL) && self.frame != 1 {
            return;
        }
        self.intensity += (self.sample(biome, frame) - self.intensity)
            * smoothing(SAMPLE_INTERVAL as f32 * frame.dt.clamp(0.001, 0.25));
        client_ambient_set(keys::SPORE_DRIFT, self.intensity, self.eddy());
    }

    fn sample(&self, biome: u8, frame: &ClientFrameData) -> f32 {
        let x = frame.player_pos[0].floor() as i32;
        let y = frame.player_pos[1].floor() as i32;
        let z = frame.player_pos[2].floor() as i32;
        let mut probes = Vec::with_capacity(1 + RING.len());
        probes.push([x, y, z]);
        for [dx, dz] in RING {
            probes.push([x + dx, y, z + dz]);
        }
        let n = probes.len() as f32;
        let hits = underground_biome_at(probes)
            .into_iter()
            .filter(|&b| b == biome)
            .count();
        hits as f32 / n
    }

    fn eddy(&self) -> [f32; 2] {
        let tau = std::f32::consts::TAU;
        let a = tau * self.clock / EDDY_PERIOD;
        let b = tau * self.clock / EDDY_PERIOD_2;
        [
            EDDY_SPEED * (0.7 * a.cos() + 0.3 * b.sin()),
            EDDY_SPEED * (0.7 * a.sin() + 0.3 * b.cos()),
        ]
    }
}

fn smoothing(dt: f32) -> f32 {
    1.0 - (-dt / SMOOTH_TAU).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_ring_grades_the_boundary() {
        let share = |px: i32| {
            let mut inside = usize::from(px <= 0);
            for [dx, _] in RING {
                inside += usize::from(px + dx <= 0);
            }
            inside as f32 / (1 + RING.len()) as f32
        };
        assert_eq!(share(-40), 1.0, "well inside is the full haze");
        assert_eq!(share(40), 0.0, "well outside is nothing");
        let edge = share(0);
        assert!(
            (0.2..=0.8).contains(&edge),
            "the boundary itself is partial, got {edge}"
        );
        let walk: Vec<f32> = (-24..=24).step_by(4).map(share).collect();
        for pair in walk.windows(2) {
            assert!(pair[1] <= pair[0], "the ramp must be monotone: {walk:?}");
        }
        assert!(
            walk.iter().filter(|v| **v > 0.0 && **v < 1.0).count() >= 3,
            "the ramp needs real intermediate steps: {walk:?}"
        );
    }

    #[test]
    fn the_eddy_wanders_and_stays_slow() {
        let mut s = Spores::default();
        let mut headings = Vec::new();
        let mut max_speed = 0.0f32;
        for step in 0..2000 {
            s.clock = step as f32 * 0.25;
            let [wx, wz] = s.eddy();
            max_speed = max_speed.max((wx * wx + wz * wz).sqrt());
            headings.push(wz.atan2(wx));
        }
        assert!(
            max_speed < 0.22,
            "the eddy must stay under the slowest fall speed, got {max_speed}"
        );
        let spread = headings
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), h| (lo.min(*h), hi.max(*h)));
        assert!(
            spread.1 - spread.0 > 5.0,
            "the eddy must sweep all headings, got {spread:?}"
        );
    }
}
