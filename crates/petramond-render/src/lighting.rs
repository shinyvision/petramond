use petramond_world::light::BlockLight6;

pub(super) const FULL_SKYLIGHT: u8 = 63;

pub(crate) const SKY_MIN: f32 = 0.02;
pub(crate) const FINAL_MIN: f32 = 0.006;

#[inline]
pub(super) fn skylight_bits(skylight: u8) -> u32 {
    (skylight.min(FULL_SKYLIGHT) as u32) << petramond_mesh::SKY_SHIFT
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DynLight {
    pub sky: u8,
    pub block: BlockLight6,
}

impl DynLight {
    pub(super) const FULL: Self = Self {
        sky: FULL_SKYLIGHT,
        block: BlockLight6::DARK,
    };

    #[inline]
    pub(super) fn new(sky: u8, block: BlockLight6) -> Self {
        Self { sky, block }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LightEnv {
    pub sky_scale: f32,
    pub sky_color: [f32; 3],
}

impl LightEnv {
    pub(super) const IDENTITY: Self = Self {
        sky_scale: 1.0,
        sky_color: [1.0, 1.0, 1.0],
    };
}

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

#[cfg(test)]
const SKY_GAMMA: i32 = 3;

#[cfg(test)]
const WGSL_BLOCK_TERM: &str = "mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk)";

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

#[inline]
pub(super) fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

#[inline]
pub(super) fn fold_self_lit(light: [f32; 3], self_lit: f32) -> [f32; 3] {
    let s = self_lit.clamp(0.0, 1.0);
    light.map(|c| c + (1.0 - c) * s)
}

#[inline]
pub(super) fn fold_tint(base: [f32; 3], light: DynLight, env: LightEnv) -> [f32; 3] {
    mul3(base, light_rgb(light, env))
}

#[inline]
pub(super) fn fold_tint_self_lit(
    base: [f32; 3],
    light: DynLight,
    env: LightEnv,
    self_lit: f32,
) -> [f32; 3] {
    mul3(base, fold_self_lit(light_rgb(light, env), self_lit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_factor(combined: u8, sky_scale: f32) -> f32 {
        channel_term(combined, sky_scale).max(FINAL_MIN)
    }

    #[test]
    fn two_channel_light_matches_the_single_channel_at_identity() {
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
        let sky = light_rgb(DynLight::new(63, BlockLight6::DARK), env);
        assert!(sky[0] < sky[2], "red must dim below blue under a blue sky");
        let torch = light_rgb(DynLight::new(0, BlockLight6::grey(63)), env);
        assert_eq!(torch[0], torch[2], "white block light is colour-neutral");
    }

    #[test]
    fn every_light_shader_spells_the_same_curve() {
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
        for level in 0..=63u8 {
            let x = level as f32 / FULL_SKYLIGHT as f32;
            assert_eq!(
                curve(x, 1.0),
                SKY_MIN + (1.0 - SKY_MIN) * x.powi(SKY_GAMMA),
                "the Rust mirror is no longer the shaders' curve at level {level}"
            );
        }
    }

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
