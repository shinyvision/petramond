//! Shared render-side lighting helpers for dynamic geometry.
//!
//! Mirrors the two-channel shader lighting in `block.wgsl` / `model3d.wgsl`:
//! sky light (dims with the sim's `sky_scale`, tints with `sky_color`) and
//! block/torch light (night-invariant), recombined with a per-channel `max`.

/// Full light on the 6-bit packed scale used by `mesh::Vertex`.
pub(super) const FULL_SKYLIGHT: u8 = 63;

// Keep in sync with `block.wgsl` / `model3d.wgsl` (dark cave floor).
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;

#[inline]
pub(super) fn skylight_bits(skylight: u8) -> u32 {
    (skylight.min(FULL_SKYLIGHT) as u32) << 23
}

/// The `Vertex::packed2` word for a block-light level (bits 0..6; the rest of
/// the word is reserved and stays zero) — the dynamic-geometry counterpart of
/// `mesh::vertex::pack_vertex2`.
#[inline]
pub(super) fn blocklight_word(blocklight: u8) -> u32 {
    blocklight.min(FULL_SKYLIGHT) as u32
}

/// A two-channel dynamic-geometry light sample (both on the 6-bit vertex
/// scale): `sky` dims with the environment, `block` (torches/furnaces) does
/// not. Sampled sim-side via `World::dynamic_light_at_world` and carried on
/// entity/instance state to the render bakes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DynLight {
    pub sky: u8,
    pub block: u8,
}

impl DynLight {
    /// Full-bright: open-sky daylight, no block light (UI icons, tests).
    pub(super) const FULL: Self = Self {
        sky: FULL_SKYLIGHT,
        block: 0,
    };

    /// A sampled `(skylight, blocklight)` pair — the instance-field constructor
    /// every dynamic bake uses.
    #[inline]
    pub(super) fn new(sky: u8, block: u8) -> Self {
        Self { sky, block }
    }
}

/// The per-frame environment inputs to the CPU light mirror: the sim's sky
/// scale (1.0 = noon) and sky colour (white = identity). One `Copy` bundle so
/// the bakers thread a single value.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct LightEnv {
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
/// night-invariant block channel).
#[inline]
fn channel_term(level6: u8, scale: f32) -> f32 {
    let x = level6.min(FULL_SKYLIGHT) as f32 / FULL_SKYLIGHT as f32;
    SKY_MIN + (1.0 - SKY_MIN) * (x * x * x * scale)
}

/// CPU mirror of the shader lighting term (`block.wgsl` / `model3d.wgsl`) for
/// dynamic geometry baked as explicit-shade vertices (mobs, dropped bbmodel
/// items, the extruded held item, particles):
/// `max(sky_term · sky_color, block_term)` per channel, floored at `FINAL_MIN`.
///
/// Identity: at `sky_scale = 1.0` + white `sky_color`, every component equals
/// the old single-channel `sky_light_factor(max(sky, block))` — the curve is
/// strictly monotone in the light level, so `max` commutes through it.
#[inline]
pub(super) fn light_rgb(light: DynLight, env: LightEnv) -> [f32; 3] {
    let sky_term = channel_term(light.sky, env.sky_scale.clamp(0.0, 1.0));
    let block_term = channel_term(light.block, 1.0);
    let mut out = [0.0f32; 3];
    for (o, c) in out.iter_mut().zip(env.sky_color) {
        *o = (sky_term * c).max(block_term).max(FINAL_MIN);
    }
    out
}

/// Component-wise RGB multiply.
#[inline]
pub(super) fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
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
                let rgb = light_rgb(DynLight { sky, block }, LightEnv::IDENTITY);
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
        let torchlit = DynLight { sky: 0, block: 60 };
        let night = LightEnv {
            sky_scale: 0.0,
            sky_color: [1.0, 1.0, 1.0],
        };
        assert_eq!(
            light_rgb(torchlit, night),
            light_rgb(torchlit, LightEnv::IDENTITY)
        );

        let skylit = DynLight { sky: 63, block: 0 };
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
        let sky = light_rgb(DynLight { sky: 63, block: 0 }, env);
        assert!(sky[0] < sky[2], "red must dim below blue under a blue sky");
        // Pure torch sample: the block term is untinted and wins every channel.
        let torch = light_rgb(DynLight { sky: 0, block: 63 }, env);
        assert_eq!(torch[0], torch[2], "block light is colour-neutral");
    }
}
