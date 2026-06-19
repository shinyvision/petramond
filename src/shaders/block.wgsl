// Block vertex/fragment shader with fog + directional face shading.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, _, _)
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv:  vec2<f32>,
    @location(2) light: f32,
    @location(3) tint: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light: f32,
    @location(2) dist: f32,
    @location(3) tint: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.light = in.light;
    out.dist = length(u.cam_pos.xyz - in.pos);
    out.tint = in.tint;
    return out;
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv);
    if (tex.a < 0.5) { discard; }
    let color = tex.rgb * in.tint * in.light;
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(vec3<f32>(color), u.fog_color.rgb, f);
    return vec4<f32>(out, 1.0);
}

@fragment
fn fs_transparent(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv);
    // Only water uses this alpha-blended pass now (leaves render fully opaque in
    // fs_opaque). Water tiles are full-alpha, so the discard is a no-op for them.
    if (tex.a < 0.5) { discard; }
    let color = tex.rgb * in.tint * in.light;
    // Water blue tint + slight transparency.
    let alpha = 0.78;
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(vec3<f32>(color), u.fog_color.rgb, f);
    return vec4<f32>(out, alpha);
}