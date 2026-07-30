//! Client presentation: the biome-wide spore haze.
//!
//! Two layers make the caverns feel alive and they answer different
//! questions. The per-block emitters on the flora say WHERE spores come from —
//! an occasional puff off a sporeshroom or a vine, deliberately rare. This
//! layer says the AIR ITSELF is full of them: a camera-following ambient
//! volume that exists everywhere in the biome, forty blocks from the nearest
//! mushroom as much as under one. It is lit by the world, so how much of it
//! you SEE tracks how much light the cavern is throwing — bright under a
//! glowcap, gone in a pocket nothing lights.
//!
//! The volume is engine-derived per frame from the bundle row; all this module
//! decides is HOW MUCH of it to show and which way the air is moving.

use mod_sdk::*;

/// The ambient bundle declared in `pack/particle_emitters.json`.
const BUNDLE: &str = "exploration:spore_drift";

/// Frames between biome samples, and between drives. The probe ring below is
/// 16 blocks wide, so a quarter second of sprinting (~1.5 blocks) cannot move
/// the answer by more than a tenth of one probe's worth — and it holds the
/// whole layer to TWO ABI crossings four times a second.
const SAMPLE_INTERVAL: u64 = 15;

/// Radius of the probe ring, blocks. Standing this far INSIDE the biome shows
/// the full haze; this far outside shows none; the boundary itself reads as a
/// gradient rather than a switch.
const PROBE_RADIUS: i32 = 16;

/// Mod-side smoothing time constant, seconds. Only there to sand the 9-step
/// staircase off the probe ring — the boundary's real softness is the ring's
/// width, and the engine adds its own ~2 s ease on top.
const SMOOTH_TAU: f32 = 1.5;

/// Peak coherent air speed, blocks/s. There is no wind underground, but a
/// dead-still volume reads as a frozen screen-space dot field, so the whole
/// body of motes wanders on a slow eddy instead. Kept far below both the
/// per-particle flutter and the fall speed so it can never read as weather.
const EDDY_SPEED: f32 = 0.12;
/// Seconds for the eddy to turn once. Long enough that the drift never looks
/// like a direction the world is blowing in.
const EDDY_PERIOD: f32 = 97.0;
/// A second, faster turn beaten against the first so the wander never repeats
/// on an obvious cycle.
const EDDY_PERIOD_2: f32 = 41.0;

/// The eight ring offsets around the camera's own column, which is probed
/// separately as the ninth.
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
    /// The pack's own underground-biome id, resolved once.
    biome: Option<u8>,
    frame: u64,
    /// Seconds since the client instance started — the eddy's clock. Frame
    /// deltas, so it cannot inherit a session-age discontinuity.
    clock: f32,
    /// Smoothed share of the probe ring inside the biome.
    intensity: f32,
}

impl Spores {
    pub(crate) fn init(&mut self) {
        self.biome = resolve_underground_biome(crate::BIOME_KEY);
        if self.biome.is_none() {
            log("exploration: no mushroom-cavern biome row; the spore haze stays off");
        }
    }

    /// Nothing here runs per frame but a counter and a clock: the volume
    /// itself is derived engine-side from the last target, and the engine
    /// eases both the intensity and the wind INTEGRAL across the gap, so
    /// re-driving it four times a second is indistinguishable from sixty.
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
        client_ambient_set(BUNDLE, self.intensity, self.eddy());
    }

    /// The share of the probe ring standing in the pack's biome. ONE ABI
    /// crossing for all nine positions — the batch is what makes a spatial
    /// ramp cost the same as a single-point test.
    fn sample(&self, biome: u8, frame: &ClientFrameData) -> f32 {
        // floor(), not truncation: at fractional negative coords `as i32`
        // probes the neighbouring column.
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

    /// The air's current: a slow two-term rotation, never a constant heading.
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

/// Exponential-ease factor for a step of `dt` seconds at [`SMOOTH_TAU`].
fn smoothing(dt: f32) -> f32 {
    1.0 - (-dt / SMOOTH_TAU).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundary must be a RAMP, not a switch: the probe ring is what
    /// makes walking out of a cavern fade the haze instead of popping it off.
    /// Pinned as arithmetic over the ring because the host call cannot run
    /// off wasm.
    #[test]
    fn the_probe_ring_grades_the_boundary() {
        // A half-plane biome: everything at x <= 0 is in. Deep inside, at the
        // edge, and well outside must be three DIFFERENT answers.
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
        // Monotone across the crossing, so the fade never reverses.
        let walk: Vec<f32> = (-24..=24).step_by(4).map(share).collect();
        for pair in walk.windows(2) {
            assert!(pair[1] <= pair[0], "the ramp must be monotone: {walk:?}");
        }
        assert!(
            walk.iter().filter(|v| **v > 0.0 && **v < 1.0).count() >= 3,
            "the ramp needs real intermediate steps: {walk:?}"
        );
    }

    /// The eddy is a WANDER: bounded well under the fall speed, and it must
    /// not settle on a heading (a constant vector slides the whole volume and
    /// reads as weather).
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
        // The slowest particle falls at 0.22 blocks/s (the bundle row); a
        // coherent drift at or above that would read as wind-blown rain.
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
