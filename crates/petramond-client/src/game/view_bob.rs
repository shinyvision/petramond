use std::f32::consts::TAU;

const CYCLES_PER_BLOCK: f32 = 0.35;

const ENVELOPE_RATE: f32 = 9.0;

const FULL_ENVELOPE_SPEED: f32 = 4.3;

const MAX_ENVELOPE: f32 = 1.3;

const MIN_SPEED: f32 = 0.35;

#[derive(Default)]
pub struct ViewBob {
    phase: f32,
    weight: f32,
}

impl ViewBob {
    pub fn advance(&mut self, dt: f32, hspeed: f32, striding: bool) {
        let dt = dt.max(0.0);
        let target = if striding && hspeed >= MIN_SPEED {
            (hspeed / FULL_ENVELOPE_SPEED).min(MAX_ENVELOPE)
        } else {
            0.0
        };
        let settle = 1.0 - (-ENVELOPE_RATE * dt).exp();
        self.weight += (target - self.weight) * settle;
        if self.weight < 1e-4 && target == 0.0 {
            self.weight = 0.0;
            self.phase = 0.0;
            return;
        }
        self.phase = (self.phase + dt * hspeed * CYCLES_PER_BLOCK).fract();
    }

    pub fn offset(&self) -> [f32; 2] {
        if self.weight == 0.0 {
            return [0.0, 0.0];
        }
        let theta = self.phase * TAU;
        [theta.sin() * self.weight, (2.0 * theta).cos() * self.weight]
    }

    pub fn stride(&self) -> (f32, f32) {
        (self.phase, self.weight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(bob: &mut ViewBob, seconds: f32, speed: f32) -> Vec<[f32; 2]> {
        let dt = 1.0 / 60.0;
        let mut out = Vec::new();
        for _ in 0..(seconds / dt) as usize {
            bob.advance(dt, speed, true);
            out.push(bob.offset());
        }
        out
    }

    #[test]
    fn both_channels_are_mean_zero_over_a_walk() {
        let mut bob = ViewBob::default();
        walk(&mut bob, 2.0, 4.3);
        let samples = walk(&mut bob, 20.0, 4.3);
        let n = samples.len() as f32;
        let mean_side = samples.iter().map(|s| s[0]).sum::<f32>() / n;
        let mean_up = samples.iter().map(|s| s[1]).sum::<f32>() / n;
        assert!(mean_side.abs() < 0.02, "side mean {mean_side}");
        assert!(mean_up.abs() < 0.02, "up mean {mean_up}");
    }

    #[test]
    fn the_head_dips_twice_per_sway() {
        let mut bob = ViewBob::default();
        walk(&mut bob, 2.0, 4.3);
        let samples = walk(&mut bob, 10.0, 4.3);
        let crossings = |f: &dyn Fn(&[f32; 2]) -> f32| {
            samples
                .windows(2)
                .filter(|w| f(&w[0]) < 0.0 && f(&w[1]) >= 0.0)
                .count()
        };
        let side = crossings(&|s| s[0]);
        let up = crossings(&|s| s[1]);
        assert!(side > 4, "not enough strides sampled: {side}");
        assert_eq!(up, side * 2, "up {up} must cycle twice per side {side}");
    }

    #[test]
    fn stopping_settles_to_rest_and_stays() {
        let mut bob = ViewBob::default();
        walk(&mut bob, 2.0, 4.3);
        assert_ne!(bob.offset(), [0.0, 0.0]);
        for _ in 0..600 {
            bob.advance(1.0 / 60.0, 0.0, false);
        }
        assert_eq!(bob.offset(), [0.0, 0.0]);
    }

    #[test]
    fn stride_strength_tracks_speed_but_is_capped() {
        let peak = |speed: f32| {
            let mut bob = ViewBob::default();
            walk(&mut bob, 3.0, speed);
            walk(&mut bob, 5.0, speed)
                .iter()
                .map(|s| s[0].abs())
                .fold(0.0f32, f32::max)
        };
        let (sneak, walk_, sprint, absurd) = (peak(2.15), peak(4.3), peak(5.6), peak(40.0));
        assert!(sneak < walk_, "{sneak} < {walk_}");
        assert!(walk_ < sprint, "{walk_} < {sprint}");
        assert!(absurd <= MAX_ENVELOPE + 1e-3, "uncapped: {absurd}");
    }
}
