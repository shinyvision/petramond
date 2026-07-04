struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    water_anim: vec4<u32>,
    sky_color: vec4<f32>,
};

struct ShaderParams {
    values: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<uniform> params: ShaderParams;
@group(1) @binding(0) var sun_tex: texture_2d<f32>;
@group(1) @binding(1) var sun_samp: sampler;
@group(1) @binding(2) var moon_tex: texture_2d<f32>;
@group(1) @binding(3) var moon_samp: sampler;

const TAU: f32 = 6.28318530718;
const ARC_TILT: f32 = 0.15;
const SUN_RADIUS: f32 = 0.078;
const MOON_RADIUS: f32 = 0.067;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>( 3.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    let p = positions[vertex_index];

    var out: VsOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

fn sky_ray(ndc: vec2<f32>) -> vec3<f32> {
    let near = u.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far = u.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let near_world = near.xyz / near.w;
    let far_world = far.xyz / far.w;
    return normalize(far_world - near_world);
}

struct SpriteUv {
    uv: vec2<f32>,
    mask: f32,
};

fn sprite_uv(ray: vec3<f32>, dir_in: vec3<f32>, radius: f32) -> SpriteUv {
    let dir = normalize(dir_in);
    var pole = vec3<f32>(0.0, 1.0, 0.0);
    if (abs(dot(dir, pole)) > 0.98) {
        pole = vec3<f32>(0.0, 0.0, 1.0);
    }
    let right = normalize(cross(pole, dir));
    let upv = cross(dir, right);
    let scale = sin(radius);
    let x = dot(ray, right) / scale;
    let y = dot(ray, upv) / scale;

    var out: SpriteUv;
    out.uv = vec2<f32>(x * 0.5 + 0.5, 0.5 - y * 0.5);
    out.mask = 0.0;
    if (abs(x) <= 1.0 && abs(y) <= 1.0 && dot(ray, dir) > cos(radius * 1.6)) {
        out.mask = 1.0;
    }
    return out;
}

fn keyed_sprite_alpha(sample: vec4<f32>, low: f32, high: f32) -> f32 {
    let brightness = max(max(sample.r, sample.g), sample.b);
    return sample.a * smoothstep(low, high, brightness);
}

@fragment
fn fs_sky(in: VsOut) -> @location(0) vec4<f32> {
    if (u.fog.w > 0.5) {
        return vec4<f32>(u.fog_color.rgb, 1.0);
    }

    let time = params.values[0];
    let light = params.values[1];
    let day_fraction = fract(time.x);
    let daylight = clamp(time.y, 0.0, 1.0);
    let phase = clamp(time.z, 0.0, 7.0);
    let sky_scale = clamp(light.x, 0.0, 1.0);
    let night = 1.0 - daylight;
    let ray = sky_ray(in.ndc);

    let horizon = u.fog_color.rgb;
    let day_zenith = vec3<f32>(0.18, 0.46, 1.0);
    let night_zenith = vec3<f32>(0.006, 0.010, 0.032);
    let zenith = mix(night_zenith, day_zenith * sky_scale * u.sky_color.rgb, daylight);
    let up = clamp(ray.y, 0.0, 1.0);
    let grad_t = smoothstep(0.0, 0.9, pow(up, 0.72));
    var color = mix(horizon, zenith, grad_t);

    let angle = TAU * day_fraction;
    let sun_dir = normalize(vec3<f32>(cos(angle), sin(angle), ARC_TILT));
    let moon_dir = normalize(vec3<f32>(-cos(angle), -sin(angle), ARC_TILT));

    let sun_sprite = sprite_uv(ray, sun_dir, SUN_RADIUS);
    let sun_sample = textureSample(sun_tex, sun_samp, sun_sprite.uv);
    let sun_alpha = keyed_sprite_alpha(sun_sample, 0.02, 0.08) * sun_sprite.mask * daylight;
    color += sun_sample.rgb * sun_alpha * 1.35;

    let moon_sprite = sprite_uv(ray, moon_dir, MOON_RADIUS);
    let phase_idx = floor(phase + 0.5);
    let phase_col = phase_idx - 4.0 * floor(phase_idx / 4.0);
    let phase_row = floor(phase_idx / 4.0);
    let moon_uv = (moon_sprite.uv + vec2<f32>(phase_col, phase_row)) * vec2<f32>(0.25, 0.5);
    let moon_sample = textureSample(moon_tex, moon_samp, moon_uv);
    let moon_alpha = keyed_sprite_alpha(moon_sample, 0.02, 0.08) * moon_sprite.mask * night;
    color += moon_sample.rgb * moon_alpha * 0.95;

    return vec4<f32>(color, 1.0);
}
