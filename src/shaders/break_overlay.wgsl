// break_overlay: the cracked-block destroy overlay.
//
// Draws the targeted block's cube faces (built CPU-side at the block's exact
// integer cell, coincident with the chunk mesh — no inflation) textured with a
// Tile::DestroyStage{0..9} grayscale crack and MULTIPLY-blends it over the world
// (result = src.rgb * dst.rgb) so the cracks darken the block face instead of
// alpha-compositing a flat overlay. group(0): Uniforms (view_proj at binding 0) +
// the shared uv_rects table (binding 1) — the SAME bind group as the block
// pipeline. group(1) is the block atlas (the destroy tiles live in it).
//
// Vertex format is the shared 28-byte mesh::Vertex. We only need uv reconstruction
// (SELECT from uv_rects — never recompute), so this is a trimmed copy of the block
// vertex stage. The crack cube is coincident with the block faces; the pipeline
// draws it depth LessEqual / no-write with a small polygon offset toward the camera
// (BREAK_DEPTH_BIAS in pipeline.rs) so the decal wins the depth tie cleanly — see
// that constant for why the offset is needed (the mesher's per-AO diagonal flip).

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<uniform> uv_rects: array<vec4<f32>, 256>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) packed: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dist: f32,
};

// Same corner mapping as block.wgsl: 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
fn corner_uv(r: vec4<f32>, corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(r.x, r.w); }
    if (corner == 1u) { return vec2<f32>(r.z, r.w); }
    if (corner == 2u) { return vec2<f32>(r.z, r.y); }
    return vec2<f32>(r.x, r.y);
}

@vertex
fn vs_break(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(in.pos, 1.0);
    let tile = in.packed & 0xFFu;
    let corner = (in.packed >> 8u) & 0x3u;
    out.uv = corner_uv(uv_rects[tile], corner);
    // World-space camera distance, for the fog fade in the fragment stage (matches
    // block.wgsl so the crack fades on the same curve as the surface it sits on).
    out.dist = length(u.cam_pos.xyz - in.pos);
    return out;
}

@fragment
fn fs_break(in: VsOut) -> @location(0) vec4<f32> {
    // The destroy tiles are grayscale + alpha: the crack pixels have alpha, the
    // rest is transparent. Under MULTIPLY blend (src.rgb * dst.rgb) we want
    // transparent texels to be WHITE (×1 = no change) and crack texels to be dark,
    // so mix toward the crack colour by its own alpha. Output alpha is unused (the
    // blend preserves the destination alpha).
    let tex = textureSample(atlas, samp, in.uv);
    var crack = mix(vec3<f32>(1.0), tex.rgb, tex.a);
    // Fog fade: the block underneath has already faded toward fog_color (mix in
    // block.wgsl), incl. the tight blue underwater fog. Fade the crack's darkening
    // toward 1.0 (multiply identity) on that same fog curve so it melts into the
    // murk with the surface instead of staying a hard dark pattern floating in fog.
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    crack = mix(crack, vec3<f32>(1.0), f);
    return vec4<f32>(crack, 1.0);
}
