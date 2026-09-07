// World resolve: supersample reduction and grade, or render-scale upsampling. UI is drawn
// afterwards so inventory art, text and targeting retain their authored colours.
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_samp: sampler;
struct PostProcess {
    darken: f32,
    desaturate: f32,
    sample_axis: f32,
    grade: f32,
};
@group(0) @binding(2) var<uniform> post: PostProcess;

const GRADE_LUMA_W: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const GRADE_CONTRAST: f32 = 0.34;
const GRADE_VIBRANCE: f32 = 0.34;
const GRADE_SHADOW_TINT: vec3<f32> = vec3<f32>(0.96, 0.98, 1.06);
const GRADE_HIGHLIGHT_TINT: vec3<f32> = vec3<f32>(1.04, 1.01, 0.95);

// Compress chroma toward the same luminance until every channel is in gamut.
// Clipping channels independently turns saturated flowers and lamps into neon.
fn grade_gamut(c: vec3<f32>) -> vec3<f32> {
    let y = clamp(dot(c, GRADE_LUMA_W), 0.0, 1.0);
    let delta = c - vec3<f32>(y);
    let hi = max(delta.r, max(delta.g, delta.b));
    let lo = min(delta.r, min(delta.g, delta.b));
    let scale = min(1.0, min((1.0 - y) / max(hi, 1e-5), y / max(-lo, 1e-5)));
    return vec3<f32>(y) + delta * scale;
}

fn grade_color(src: vec3<f32>) -> vec3<f32> {
    let y = dot(src, GRADE_LUMA_W);
    // Shape perceived brightness, then scale RGB together: a per-channel
    // contrast curve changes the hue of the game's already painted textures.
    let p = sqrt(clamp(y, 0.0, 1.0));
    let curve = p * p * (3.0 - 2.0 * p);
    let shaped = mix(p, curve, GRADE_CONTRAST * smoothstep(0.012, 0.10, y));
    var c = src * (shaped * shaped / max(y, 1e-5));
    let luma = dot(c, GRADE_LUMA_W);
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    let saturation = (hi - lo) / max(hi, 1e-5);
    let vibrance = GRADE_VIBRANCE * (1.0 - saturation) * smoothstep(0.015, 0.12, luma);
    c = mix(vec3<f32>(luma), c, 1.0 + vibrance);
    let tone = smoothstep(0.10, 0.72, luma);
    let tint = mix(GRADE_SHADOW_TINT, GRADE_HIGHLIGHT_TINT, tone);
    c *= mix(vec3<f32>(1.0), tint / dot(tint, GRADE_LUMA_W), smoothstep(0.012, 0.08, luma));
    return grade_gamut(c);
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_grade(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>( 3.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    var out: VsOut;
    let p = positions[vertex_index];
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_grade(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(scene, scene_samp, in.uv, 0.0).rgb;
    // At 2x per axis the center fetch averages exactly this output pixel's
    // four scene texels. At 4x, four bilinear fetches cover its sixteen texels.
    // Nothing outside that pixel footprint is blended into a sharp boundary.
    if post.sample_axis > 2.0 {
        let offset = 1.0 / vec2<f32>(textureDimensions(scene));
        c = (textureSampleLevel(scene, scene_samp, in.uv + vec2<f32>(-offset.x, -offset.y), 0.0).rgb
            + textureSampleLevel(scene, scene_samp, in.uv + vec2<f32>(offset.x, -offset.y), 0.0).rgb
            + textureSampleLevel(scene, scene_samp, in.uv + vec2<f32>(-offset.x, offset.y), 0.0).rgb
            + textureSampleLevel(scene, scene_samp, in.uv + offset, 0.0).rgb) * 0.25;
    }
    if post.grade > 0.5 {
        c = grade_color(c);
    }
    c = mix(c, vec3<f32>(dot(c, GRADE_LUMA_W)), post.desaturate);
    return vec4<f32>(c * (1.0 - post.darken), 1.0);
}
