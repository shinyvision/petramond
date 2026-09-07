// Shared painted-light ramps. Continuous transitions keep moving daylight and
// interpolated light pools from popping as they cross a tone boundary.
struct CelStages {
    edges: vec2<f32>,
    widths: vec2<f32>,
    tones: vec3<f32>,
    smooth_mix: f32,
}

const CEL_SUN: CelStages = CelStages(
    vec2<f32>(0.04, 0.46), vec2<f32>(0.28, 0.36),
    vec3<f32>(0.0, 0.46, 1.0), 0.16,
);
const CEL_LIGHT: CelStages = CelStages(
    vec2<f32>(0.36, 0.70), vec2<f32>(0.24, 0.28),
    vec3<f32>(0.24, 0.62, 1.0), 0.65,
);

fn cel_band(s: CelStages, strength: f32) -> f32 {
    let x = clamp(strength, 0.0, 1.0);
    let steps = smoothstep(s.edges, s.edges + s.widths, vec2<f32>(x));
    let painted = s.tones.x + (s.tones.y - s.tones.x) * steps.x
        + (s.tones.z - s.tones.y) * steps.y;
    return mix(painted, x, s.smooth_mix);
}

// A scalar ratio preserves coloured emitters; the dark fade preserves the
// original cave gradients and the distinction between lit and unlit surfaces.
fn cel_light_ratio(drive: f32) -> f32 {
    let strength = smoothstep(0.30, 0.58, drive) * 0.45;
    return mix(1.0, cel_band(CEL_LIGHT, drive) / max(drive, 1e-4), strength);
}

fn cel_rim(
    n: vec3<f32>, view_dir: vec3<f32>, light: vec3<f32>,
    sun_dir: vec3<f32>, exposure: f32,
) -> vec3<f32> {
    let edge = 1.0 - abs(dot(n, view_dir));
    let e2 = edge * edge;
    // A sun-facing gate prevents every grazing wall from looking emissive.
    let sunward = smoothstep(-0.10, 0.65, dot(n, sun_dir));
    return light * vec3<f32>(1.0, 0.95, 0.84)
        * (e2 * e2 * 0.055 * sunward * exposure);
}
