// Analytic fluid sheen (a medium row's `sheen`): no scene reads, extra passes
// or displaced collision surface. Broad waves fade with distance so the
// horizon cannot sparkle.
struct SheenSurface {
    color: vec3<f32>,
    alpha: f32,
}

fn fluid_sheen(
    color: vec3<f32>, alpha: f32, local_pos: vec3<f32>, origin: vec3<i32>,
    view_dir: vec3<f32>, dist: f32, time: f32, exposure: f32,
    sky_scale: f32, sky_color: vec3<f32>, haze: vec3<f32>, body_tint: vec3<f32>,
    sun_dir: vec3<f32>, daylight: f32,
) -> SheenSurface {
    // Integral wave frequencies make the origin's 64-block wrap seamless.
    let p = local_pos.xz + vec2<f32>(origin.xz % vec2<i32>(64));
    let wave = vec2<f32>(
        sin(dot(p, vec2<f32>(0.09817477, 0.19634954)) + time * 0.7),
        sin(dot(p, vec2<f32>(-0.19634954, 0.09817477)) + time * 0.5),
    ) * (0.028 * (1.0 - smoothstep(12.0, 70.0, dist)));
    let n = normalize(vec3<f32>(wave.x, 1.0, wave.y));
    let facing = clamp(dot(n, -view_dir), 0.0, 1.0);
    let edge = 1.0 - facing;
    let e2 = edge * edge;
    let fresnel = 0.04 + 0.96 * e2 * e2 * edge;
    let ray = reflect(view_dir, n);
    let horizon = atmosphere_haze_color(ray, haze, sun_dir, daylight);
    let sky = vec3<f32>(0.18, 0.43, 0.72) * sky_color * sky_scale;
    // Let the biome own the body hue, including the reflected sky. A bright
    // horizon mixed into every viewing angle turns blue water into pale milk.
    let reflection = mix(horizon, sky, smoothstep(0.02, 0.85, ray.y))
        * mix(vec3<f32>(1.0), body_tint, 0.65);
    let sky_access = exposure * exposure;
    var result: SheenSurface;
    result.color = mix(color, reflection,
        (0.015 + 0.18 * fresnel) * sky_access);
    // Grazing angles reflect more and transmit less.
    result.alpha = mix(alpha, 1.0, 0.18 + 0.36 * fresnel);
    return result;
}
