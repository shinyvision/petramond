//! Speed-coupled field of view: the camera widens as the body gets faster.
//!
//! PRESENTATION ONLY, exactly like `view_bob`: a per-frame retarget of
//! `Camera::fov_y` that never reaches the simulation, a raycast, or anything
//! a server validates.
//!
//! The signal is the WISHED land speed (`Player::wish_speed`) over the base
//! walk — the speed movement would apply right now, not the velocity. That
//! one number is the whole design: sprint raises the selection only while
//! the body deliberately moves (a held key over planted feet wishes
//! nothing), a speed effect scales it moving or not (it is body state, not a
//! wish), and sprinting under a speed effect compounds both multiplicatively
//! — so there is no per-cause wiring here and a new source of speed widens
//! the view without this module changing. Sneak's slower selection narrows
//! the view a touch by the same rule, likewise only while moving.

/// Fraction of the speed surplus that becomes FOV:
/// `multiplier = 1 + GAIN · (ratio − 1)`. At 70° base: sprint (ratio ≈ 1.3)
/// reads as a slight widening (~+3°), a speed effect (1.5) a bit more
/// (~+5°), sprinting under one (~1.95) noticeably more (~+10°).
const GAIN: f32 = 0.15;
/// Multiplier floor/ceiling. The ceiling is the fisheye guard: effect rows
/// may scale speed up to 5×, and an unbounded mapping would swim the view.
/// The floor keeps the sneak narrowing subtle.
const MIN_MULT: f32 = 0.92;
const MAX_MULT: f32 = 1.30;
/// Exponential settle rate (per second): fast enough that starting a sprint
/// reads as a surge, slow enough that the FOV never snaps.
const SETTLE_SPEED: f32 = 8.0;

/// The smoothed FOV multiplier. One instance per local view.
pub struct SpeedFov {
    /// The authored FOV (radians) the camera was built with — every frame's
    /// output is this base times the eased multiplier, so retargeting can
    /// never compound.
    base_fov_y: f32,
    mult: f32,
}

impl SpeedFov {
    pub fn new(base_fov_y: f32) -> Self {
        Self { base_fov_y, mult: 1.0 }
    }

    /// Advance one frame toward `speed_ratio` (selected land speed over base
    /// walk; `1.0` = an unmodified walking body = the authored FOV exactly).
    pub fn advance(&mut self, dt: f32, speed_ratio: f32) {
        let target = (1.0 + GAIN * (speed_ratio - 1.0)).clamp(MIN_MULT, MAX_MULT);
        let settle = 1.0 - (-SETTLE_SPEED * dt.max(0.0)).exp();
        self.mult += (target - self.mult) * settle;
        // Land exactly on the target once the ease is visually over, so a
        // body back at base speed renders the authored FOV, not an epsilon
        // off it forever.
        if (self.mult - target).abs() < 1e-4 {
            self.mult = target;
        }
    }

    /// This frame's FOV (radians).
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

    /// The contract behind the whole feature: every cause of speed arrives as
    /// one ratio, and more speed is always more FOV — sprint widens, a speed
    /// effect widens more, and both together widen the most. A plain walk is
    /// the AUTHORED fov exactly (not an epsilon off it), so the feature is
    /// invisible until something is actually faster.
    #[test]
    fn more_speed_is_always_more_fov_and_a_walk_is_exactly_base() {
        let base = 70f32.to_radians();
        assert_eq!(settled(1.0), base);
        let (sprint, effect, both) = (settled(1.3), settled(1.5), settled(1.3 * 1.5));
        assert!(base < sprint, "{base} < {sprint}");
        assert!(sprint < effect, "{sprint} < {effect}");
        assert!(effect < both, "{effect} < {both}");
        // Sneak's slower selection narrows, mildly and bounded.
        let sneak = settled(0.5);
        assert!(MIN_MULT * base <= sneak && sneak < base, "{sneak}");
    }

    /// A mod-boosted speed (effect rows go to 5×) must not fisheye the
    /// camera, and the widening must fully let go: after the boost ends the
    /// view settles back to exactly the authored FOV.
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
