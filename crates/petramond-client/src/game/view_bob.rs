//! View bob: the small sway a walking body gives the first-person camera.
//!
//! PRESENTATION ONLY. Nothing here reaches the simulation — the sim eye
//! (`player::EYE`) is untouched, exactly like the step-up glide and the sneak
//! eye drop this composes with in `sync_camera_to_player_eye`. It follows that
//! bob can never move a raycast target, a movement claim, or anything a server
//! validates: it is a render offset on `Camera::pos` and nothing else.
//!
//! The output is NORMALIZED (`side`/`up` in roughly `-1..=1`, envelope folded
//! in) rather than in blocks, because two consumers want the same signal at
//! very different scales: the camera in world units, and the first-person hand
//! in view-space units. Amplitude belongs to each consumer; PHASE belongs here.

use std::f32::consts::TAU;

/// Stride cycles per block travelled — the body pose's own walk constant, so
/// the camera sways in step with the legs a third-person view would draw.
const CYCLES_PER_BLOCK: f32 = 0.35;

/// How fast the walk envelope eases in and out (per second). Fast enough that
/// the sway starts with the stride, slow enough that a stop settles rather than
/// snapping the camera straight.
const ENVELOPE_RATE: f32 = 9.0;

/// Speed (blocks/s) the envelope reaches `1.0` at — a normal walk. Slower gaits
/// bob proportionally less (a sneak barely at all), and a sprint bobs harder,
/// capped by [`MAX_ENVELOPE`].
const FULL_ENVELOPE_SPEED: f32 = 4.3;

/// Ceiling on the speed scaling, so a sprint reads as a stronger stride without
/// a mod-boosted or current-carried body swinging the camera wildly.
const MAX_ENVELOPE: f32 = 1.3;

/// Below this speed (blocks/s) a body is not striding at all.
const MIN_SPEED: f32 = 0.35;

/// The walking camera sway. One instance per local view.
#[derive(Default)]
pub struct ViewBob {
    /// Stride phase in CYCLES (one full left-right sway per cycle), kept in
    /// `0..1` so it never loses precision on a long walk.
    phase: f32,
    /// Eased stride strength, `0` standing … [`MAX_ENVELOPE`] sprinting.
    weight: f32,
}

impl ViewBob {
    /// Advance one frame. `striding` is the caller's "this body is walking on
    /// the ground under its own power" verdict — false for spectators, riders,
    /// sleepers and anything airborne, all of which simply ease back to rest.
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
            // Rest at phase 0, where both channels are at an extreme but the
            // envelope is zero — so the next stride starts from a settled
            // camera instead of resuming mid-sway.
            self.phase = 0.0;
            return;
        }
        // Phase tracks DISTANCE, not time: the sway is a property of the
        // stride, so it slows with the body instead of running on a clock.
        self.phase = (self.phase + dt * hspeed * CYCLES_PER_BLOCK).fract();
    }

    /// `(side, up)` — the normalized sway, envelope folded in.
    ///
    /// The two channels are deliberately at different RATES, which is what
    /// makes this read as walking rather than as wobbling: the body sways
    /// left-right once per stride (`sin θ`) while the head dips once per
    /// FOOTFALL, twice as often (`cos 2θ`). The relative phase is what matters
    /// — the head is lowest exactly when the sway is at an extreme, i.e. when a
    /// foot is planted. Both are mean-zero over a stride, so walking never
    /// shifts the resting eye height or centre.
    ///
    /// `cos 2θ` rather than `|sin θ|` (the classic spelling) because the
    /// absolute value has a derivative kink at every footfall, which reads as a
    /// tick in the motion at low amplitudes.
    pub fn offset(&self) -> [f32; 2] {
        if self.weight == 0.0 {
            return [0.0, 0.0];
        }
        let theta = self.phase * TAU;
        [theta.sin() * self.weight, (2.0 * theta).cos() * self.weight]
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

    /// The invariant that keeps bob from being felt as a bias rather than a
    /// motion: over whole strides both channels average to zero, so a walking
    /// player's eye sits exactly where a standing player's does. A future
    /// "simplification" to a dip-only vertical (`-|sin|`) silently lowers the
    /// camera the whole time you walk.
    #[test]
    fn both_channels_are_mean_zero_over_a_walk() {
        let mut bob = ViewBob::default();
        // Long enough for the envelope to settle, then measure a whole number
        // of strides at a steady speed.
        walk(&mut bob, 2.0, 4.3);
        let samples = walk(&mut bob, 20.0, 4.3);
        let n = samples.len() as f32;
        let mean_side = samples.iter().map(|s| s[0]).sum::<f32>() / n;
        let mean_up = samples.iter().map(|s| s[1]).sum::<f32>() / n;
        assert!(mean_side.abs() < 0.02, "side mean {mean_side}");
        assert!(mean_up.abs() < 0.02, "up mean {mean_up}");
    }

    /// The head must dip TWICE per left-right sway — one dip per footfall. A
    /// vertical channel at the same rate as the lateral one reads as leaning,
    /// not walking.
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

    /// Standing still settles to exactly zero and STAYS there — a residual
    /// offset would park the camera off-centre for the rest of the session.
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

    /// A sprint bobs harder than a walk, and a mod-boosted or current-carried
    /// body cannot swing the camera arbitrarily far.
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
