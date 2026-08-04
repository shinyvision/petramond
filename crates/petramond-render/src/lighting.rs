//! Shared render-side lighting helpers for dynamic geometry.
//!
//! Mirrors the two-channel shader lighting in `block.wgsl` / `model3d.wgsl`:
//! sky light (dims with the sim's `sky_scale`, tints with `sky_color`) and
//! block light (night-invariant, and COLOURED — each channel rides its own
//! curve), recombined with a per-channel `max`.

use petramond_world::light::BlockLight6;

/// Full light on the 6-bit packed scale used by `mesh::Vertex`.
pub(super) const FULL_SKYLIGHT: u8 = 63;

// The light curve is spelled FOUR times — here, and by hand in `block.wgsl`,
// `model3d.wgsl` and `mob.wgsl`, which no Rust code can execute. Drift between
// them is invisible until something renders wrong, so
// `every_light_shader_spells_the_same_curve` re-reads the three sources and
// pins them against these.
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;

#[inline]
pub(super) fn skylight_bits(skylight: u8) -> u32 {
    (skylight.min(FULL_SKYLIGHT) as u32) << petramond_mesh::SKY_SHIFT
}

/// A two-channel dynamic-geometry light sample (both on the 6-bit vertex
/// scale): `sky` dims with the environment, `block` (torches, glowing plants)
/// does not and carries its own COLOUR. Sampled sim-side via
/// `World::dynamic_light_at_world` and carried on entity/instance state to the
/// render bakes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DynLight {
    pub sky: u8,
    pub block: BlockLight6,
}

impl DynLight {
    /// Full-bright: open-sky daylight, no block light (UI icons, tests).
    pub(super) const FULL: Self = Self {
        sky: FULL_SKYLIGHT,
        block: BlockLight6::DARK,
    };

    /// A sampled `(skylight, blocklight)` pair — the instance-field constructor
    /// every dynamic bake uses.
    #[inline]
    pub(super) fn new(sky: u8, block: BlockLight6) -> Self {
        Self { sky, block }
    }
}

/// The per-frame environment inputs to the CPU light mirror: the sim's sky
/// scale (1.0 = noon) and sky colour (white = identity). One `Copy` bundle so
/// the bakers thread a single value.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LightEnv {
    pub sky_scale: f32,
    pub sky_color: [f32; 3],
}

impl LightEnv {
    /// Identity environment (noon, white sky) — UI/icon paths.
    pub(super) const IDENTITY: Self = Self {
        sky_scale: 1.0,
        sky_color: [1.0, 1.0, 1.0],
    };
}

/// Scalar mirror of one lighting channel's curve:
/// `SKY_MIN + (1 - SKY_MIN) * level³ * scale` (pass `scale = 1.0` for the
/// night-invariant block channel). Every channel rides the SKY_MIN floor
/// independently, so a saturated colour's weak channels bottom out at the cave
/// floor rather than below it.
#[inline]
fn channel_term(level6: u8, scale: f32) -> f32 {
    curve(
        level6.min(FULL_SKYLIGHT) as f32 / FULL_SKYLIGHT as f32,
        scale,
    )
}

#[inline]
fn curve(x: f32, scale: f32) -> f32 {
    SKY_MIN + (1.0 - SKY_MIN) * (x * x * x * scale)
}

/// The exponent [`curve`] hardcodes as `x*x*x` — the value the shaders must
/// declare, and the reason they may not reach for a transcendental `pow` on the
/// three block channels.
#[cfg(test)]
const SKY_GAMMA: i32 = 3;

/// The block term exactly as every light-decoding shader must spell it: the
/// curve applied PER CHANNEL, each riding its own `SKY_MIN` floor. A scalar
/// floor with the colour multiplied in afterwards would drive a saturated
/// purple's green below the unlit-cave floor — a black-green cast darker than
/// no light at all.
#[cfg(test)]
const WGSL_BLOCK_TERM: &str = "mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk)";

/// CPU mirror of the shader lighting term (`block.wgsl` / `model3d.wgsl`) for
/// dynamic geometry baked as explicit-shade vertices (mobs, dropped bbmodel
/// items, the extruded held item, particles):
/// `max(sky_term · sky_color, block_term)` per channel, floored at `FINAL_MIN`.
/// The block term is per channel too, each riding its own `SKY_MIN` floor.
///
/// Identity: at `sky_scale = 1.0` + white `sky_color`, every component equals
/// the old single-channel `sky_light_factor(max(sky, block))` — the curve is
/// strictly monotone in the light level, so `max` commutes through it.
#[inline]
pub(super) fn light_rgb(light: DynLight, env: LightEnv) -> [f32; 3] {
    let sky_term = channel_term(light.sky, env.sky_scale.clamp(0.0, 1.0));
    let block = light.block.fractions();
    let mut out = [0.0f32; 3];
    for (i, (o, c)) in out.iter_mut().zip(env.sky_color).enumerate() {
        *o = (sky_term * c).max(curve(block[i], 1.0)).max(FINAL_MIN);
    }
    out
}

/// Component-wise RGB multiply.
#[inline]
pub(super) fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// Mix a sampled light factor toward full bright by `self_lit` (`0..=1`) — the
/// fraction of its brightness a particle provides itself. `0` returns the
/// sample unchanged, `1` returns full bright (the old `fullbright` row flag),
/// and an intermediate value keeps the sample's RESPONSE to the room while
/// flooring it: in the dark the factor lands at `self_lit` rather than on the
/// `SKY_MIN` cave floor.
#[inline]
pub(super) fn fold_self_lit(light: [f32; 3], self_lit: f32) -> [f32; 3] {
    let s = self_lit.clamp(0.0, 1.0);
    light.map(|c| c + (1.0 - c) * s)
}

/// The standard dynamic-bake tint fold: `base * light_rgb(light, env)` per
/// channel — the sampled two-channel light rides the vertex TINT while the
/// vertex `shade` keeps only the directional term.
#[inline]
pub(super) fn fold_tint(base: [f32; 3], light: DynLight, env: LightEnv) -> [f32; 3] {
    mul3(base, light_rgb(light, env))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-split scalar term: `mix(SKY_MIN, 1, combined³ · scale)` floored at
    /// `FINAL_MIN` — kept here only as the identity reference.
    fn legacy_factor(combined: u8, sky_scale: f32) -> f32 {
        channel_term(combined, sky_scale).max(FINAL_MIN)
    }

    #[test]
    fn two_channel_light_matches_the_single_channel_at_identity() {
        // At sky_scale 1.0 + white sky colour, the split channels must reproduce
        // the old max-folded single channel bit-for-bit for every level pair.
        for sky in 0..=63u8 {
            for block in 0..=63u8 {
                let rgb = light_rgb(
                    DynLight {
                        sky,
                        block: BlockLight6::grey(block as u32),
                    },
                    LightEnv::IDENTITY,
                );
                let expect = legacy_factor(sky.max(block), 1.0);
                for c in rgb {
                    assert_eq!(
                        c.to_bits(),
                        expect.to_bits(),
                        "identity mismatch at sky={sky} block={block}"
                    );
                }
            }
        }
    }

    #[test]
    fn block_channel_is_night_invariant() {
        // Dimming the sky must not dim a torch-lit sample: the block term wins the
        // max and stays put while the sky-only sample collapses to the cave floor.
        let torchlit = DynLight {
            sky: 0,
            block: BlockLight6::grey(60),
        };
        let night = LightEnv {
            sky_scale: 0.0,
            sky_color: [1.0, 1.0, 1.0],
        };
        assert_eq!(
            light_rgb(torchlit, night),
            light_rgb(torchlit, LightEnv::IDENTITY)
        );

        let skylit = DynLight {
            sky: 63,
            block: BlockLight6::DARK,
        };
        let day = light_rgb(skylit, LightEnv::IDENTITY);
        let dark = light_rgb(skylit, night);
        assert!(dark[0] < day[0], "sky-only light must dim with the scale");
    }

    #[test]
    fn sky_color_tints_only_the_sky_term() {
        let env = LightEnv {
            sky_scale: 1.0,
            sky_color: [0.75, 0.82, 1.0],
        };
        // Pure sky sample: tinted per channel.
        let sky = light_rgb(DynLight::new(63, BlockLight6::DARK), env);
        assert!(sky[0] < sky[2], "red must dim below blue under a blue sky");
        // Pure WHITE torch sample: the block term is untinted and wins every
        // channel.
        let torch = light_rgb(DynLight::new(0, BlockLight6::grey(63)), env);
        assert_eq!(torch[0], torch[2], "white block light is colour-neutral");
    }

    /// The light curve lives in FOUR hand-written copies (this module plus
    /// three WGSL sources). Nothing executes the WGSL from Rust, so a constant
    /// that drifts on one side just renders slightly wrong somewhere. Re-read
    /// the shaders and pin all three constants — and the exact spelling of the
    /// PER-CHANNEL block term, which is what keeps a saturated hue's weak
    /// channels on the cave floor instead of below it.
    #[test]
    fn every_light_shader_spells_the_same_curve() {
        /// `const NAME: f32 = <literal>;` as the WGSL sources declare it.
        fn wgsl_const(src: &str, name: &str) -> f32 {
            let needle = format!("const {name}: f32 = ");
            let at = src
                .find(&needle)
                .unwrap_or_else(|| panic!("no `{name}` declaration"))
                + needle.len();
            let end = src[at..]
                .find(';')
                .unwrap_or_else(|| panic!("unterminated `{name}`"));
            src[at..at + end]
                .trim()
                .parse()
                .unwrap_or_else(|e| panic!("`{name}` is not a float literal: {e}"))
        }
        for (name, src) in [
            ("block.wgsl", include_str!("../shaders/block.wgsl")),
            ("model3d.wgsl", include_str!("../shaders/model3d.wgsl")),
            ("mob.wgsl", include_str!("../shaders/mob.wgsl")),
        ] {
            assert_eq!(wgsl_const(src, "SKY_MIN"), SKY_MIN, "{name} SKY_MIN");
            assert_eq!(wgsl_const(src, "FINAL_MIN"), FINAL_MIN, "{name} FINAL_MIN");
            assert_eq!(
                wgsl_const(src, "SKY_GAMMA"),
                SKY_GAMMA as f32,
                "{name} SKY_GAMMA"
            );
            assert!(
                src.contains(WGSL_BLOCK_TERM),
                "{name} no longer floors the block term PER CHANNEL as \
                 `{WGSL_BLOCK_TERM}` — a saturated colour's weak channels can \
                 now render darker than an unlit cave"
            );
        }
        // ... and this module's own copy is that same curve, so pinning the
        // shaders against it is meaningful.
        for level in 0..=63u8 {
            let x = level as f32 / FULL_SKYLIGHT as f32;
            assert_eq!(
                curve(x, 1.0),
                SKY_MIN + (1.0 - SKY_MIN) * x.powi(SKY_GAMMA),
                "the Rust mirror is no longer the shaders' curve at level {level}"
            );
        }
    }

    /// A coloured block light must colour a dynamic surface the same way it
    /// colours terrain — per channel — and every channel must still ride the
    /// SKY_MIN cave floor, or a saturated hue renders DARKER than an unlit cave
    /// in its weak channels.
    #[test]
    fn coloured_block_light_rides_the_cave_floor_per_channel() {
        let purple = light_rgb(
            DynLight::new(0, BlockLight6::new(40, 0, 63)),
            LightEnv::IDENTITY,
        );
        let unlit = light_rgb(DynLight::new(0, BlockLight6::DARK), LightEnv::IDENTITY);
        assert!(purple[2] > purple[0], "blue must outshine red");
        assert!(purple[0] > purple[1], "red must outshine the dead green");
        for c in 0..3 {
            assert!(
                purple[c] >= unlit[c],
                "channel {c} fell below the unlit floor: {purple:?} vs {unlit:?}"
            );
        }
    }
}
