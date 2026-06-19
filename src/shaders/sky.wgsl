// Full-screen sky background. The horizon starts at the current biome fog colour
// so distant terrain and sky meet cleanly; upward view rays blend into a deeper
// blue.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, time, underwater)
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

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

@fragment
fn fs_sky(in: VsOut) -> @location(0) vec4<f32> {
    if (u.fog.w > 0.5) {
        return vec4<f32>(u.fog_color.rgb, 1.0);
    }

    let near = u.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = u.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let near_world = near.xyz / near.w;
    let far_world = far.xyz / far.w;
    let ray = normalize(far_world - near_world);

    let horizon = u.fog_color.rgb;
    let zenith = vec3<f32>(0.18, 0.46, 1.0);
    let up = clamp(ray.y, 0.0, 1.0);
    let t = smoothstep(0.0, 0.85, pow(up, 0.72));
    let color = mix(horizon, zenith, t);
    return vec4<f32>(color, 1.0);
}
