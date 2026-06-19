// Selection outline: draws a black wireframe cube (LineList) at the targeted
// block. Reuses the block pipeline's Uniforms buffer (view_proj only); the
// struct layout must stay byte-identical to render::Uniforms / block.wgsl.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>,
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_outline(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return u.view_proj * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_outline() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
