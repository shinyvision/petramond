const GAIN: f32 = 0.15;
const MIN_MULT: f32 = 0.92;
const MAX_MULT: f32 = 1.30;
const SETTLE_SPEED: f32 = 8.0;

pub struct SpeedFov {
    base_fov_y: f32,
    mult: f32,
}

impl SpeedFov {
    pub fn new(base_fov_y: f32) -> Self {
        Self {
            base_fov_y,
            mult: 1.0,
        }
    }

    pub fn advance(&mut self, dt: f32, speed_ratio: f32) {
        let target = (1.0 + GAIN * (speed_ratio - 1.0)).clamp(MIN_MULT, MAX_MULT);
        let settle = 1.0 - (-SETTLE_SPEED * dt.max(0.0)).exp();
        self.mult += (target - self.mult) * settle;
        if (self.mult - target).abs() < 1e-4 {
            self.mult = target;
        }
    }

    pub fn fov_y(&self) -> f32 {
        self.base_fov_y * self.mult
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(ratio: f32) -> f32 {
        let mut fov = SpeedFov::new(70f32.to_radians());
        for _ in 0..600 {
            fov.advance(1.0 / 60.0, ratio);
        }
        fov.fov_y()
    }

    #[test]
    fn more_speed_is_always_more_fov_and_a_walk_is_exactly_base() {
        let base = 70f32.to_radians();
        assert_eq!(settled(1.0), base);
        let (sprint, effect, both) = (settled(1.3), settled(1.5), settled(1.3 * 1.5));
        assert!(base < sprint, "{base} < {sprint}");
        assert!(sprint < effect, "{sprint} < {effect}");
        assert!(effect < both, "{effect} < {both}");
        let sneak = settled(0.5);
        assert!(MIN_MULT * base <= sneak && sneak < base, "{sneak}");
    }

    #[test]
    fn absurd_speed_is_capped_and_the_widening_lets_go() {
        let base = 70f32.to_radians();
        assert_eq!(settled(6.5), base * MAX_MULT);
        let mut fov = SpeedFov::new(base);
        for _ in 0..120 {
            fov.advance(1.0 / 60.0, 6.5);
        }
        for _ in 0..600 {
            fov.advance(1.0 / 60.0, 1.0);
        }
        assert_eq!(fov.fov_y(), base);
    }
}
