use super::*;

const SHADER: &str = include_str!("../../weather/pack/shaders/clouds.wgsl");

fn shader_fmix(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^ (h >> 16)
}

fn shader_corner(ix: u32, iz: u32, seed: u32) -> f32 {
    let h = shader_fmix(ix.wrapping_mul(0x9E37_79B9) ^ iz.wrapping_mul(0x85EB_CA6B) ^ seed);
    (h >> 8) as f32 / 16_777_216.0
}

fn shader_vnoise(px: f32, pz: f32, period: u32, seed: u32) -> f32 {
    let fx = px.floor();
    let fz = pz.floor();
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let tx = smooth(px - fx);
    let tz = smooth(pz - fz);
    let mask = period - 1;
    let ix = (fx as i32 as u32) & mask;
    let iz = (fz as i32 as u32) & mask;
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    mix(
        mix(
            shader_corner(ix, iz, seed),
            shader_corner((ix + 1) & mask, iz, seed),
            tx,
        ),
        mix(
            shader_corner(ix, (iz + 1) & mask, seed),
            shader_corner((ix + 1) & mask, (iz + 1) & mask, seed),
            tx,
        ),
        tz,
    )
}

fn shader_fbm(q: [f32; 2], off: [f32; 2], base: u32, seed: u32) -> f32 {
    let n0 = shader_vnoise(q[0] - off[0], q[1] - off[1], base, seed);
    let n1 = shader_vnoise(
        (q[0] - 2.0 * off[0]) * 2.0,
        (q[1] - 2.0 * off[1]) * 2.0,
        base * 2,
        seed ^ 0x9E37_79B9,
    );
    let n2 = shader_vnoise(
        (q[0] - off[0]) * 4.0,
        (q[1] - off[1]) * 4.0,
        base * 4,
        seed ^ 0x3C6E_F372,
    );
    (n0 + 0.5 * n1 + 0.25 * n2) / 1.75
}

fn shader_sheet(xz: [f32; 2], p: &FieldParams, salt: u32, feature: f32, advect: f32) -> f32 {
    let q = [xz[0] / feature, xz[1] / feature];
    let off = [advect * p.off[0] / feature, advect * p.off[1] / feature];
    let base = (WRAP / feature) as u32;
    let a = shader_fbm(q, off, base, p.seed ^ shader_fmix(p.epoch) ^ salt);
    let b = shader_fbm(
        q,
        off,
        base,
        p.seed ^ shader_fmix(p.epoch.wrapping_add(1)) ^ salt,
    );
    let n = a + (b - a) * p.epoch_frac.clamp(0.0, 1.0);
    let lo = 1.0 - p.storm;
    ((n - lo) / (1.0 - lo).max(0.001)).clamp(0.0, 1.0)
}

fn shader_coverage(xz: [f32; 2], p: &FieldParams) -> f32 {
    (shader_sheet(xz, p, 0, FEATURE_SIZE, 1.0)
        + shader_sheet(xz, p, SHEET_B_SALT, SHEET_B_FEATURE, SHEET_B_ADVECT))
    .clamp(0.0, 1.0)
}

#[test]
fn generated_shader_constants_and_coverage_math_match_the_sim() {
    let constants = format!(
        "const WRAP: f32 = {:?};\nconst SHEET_B_FEATURE: f32 = {:?};\nconst SHEET_B_ADVECT: f32 = {:?};\nconst SHEET_B_SALT: u32 = 0x{:08X}u;\nconst RAIN_RAMP: f32 = {:?};",
        WRAP, SHEET_B_FEATURE, SHEET_B_ADVECT, SHEET_B_SALT, RAIN_RAMP,
    );
    assert!(
        SHADER.contains(&constants),
        "weather shader constants were not generated"
    );
    for expression in [
        "h *= 0x85EBCA6Bu",
        "h *= 0xC2B2AE35u",
        "(q.x - 2.0 * o.x) * 2.0",
        "seed ^ fmix32(epoch + 1u) ^ salt",
        "return clamp(ca + cb, 0.0, 1.0)",
    ] {
        assert!(
            SHADER.contains(expression),
            "weather shader changed: {expression}"
        );
    }
    for seed in [0, 7, 0xDEAD_BEEF] {
        let p = FieldParams {
            off: [123.25, 2101.5],
            storm: 0.61,
            seed,
            epoch: 17,
            epoch_frac: 0.37,
        };
        for i in 0..128 {
            let x = (i * 89 % 4000) as f32 + 0.25;
            let z = (i * 173 % 3700) as f32 + 0.75;
            let cpu = coverage(f64::from(x), f64::from(z), &p);
            let shader = shader_coverage([x, z], &p);
            assert!(
                (cpu - shader).abs() < 0.0002,
                "{seed:#x} {x},{z}: {cpu} vs {shader}"
            );
        }
    }
}
