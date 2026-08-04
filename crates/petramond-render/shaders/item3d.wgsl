// item3d: first-person EXTRUDED held item (flowers / future tools).
//
// A flat 16x16 item sprite given real voxel depth on the CPU (see
// render::item_model::build_extruded_item): a front + back textured face plus
// side-wall quads along the alpha boundary, so the silhouette reads as a 3D
// stepped slab held at an angle. Unlike model3d.wgsl (which can only SELECT
// whole-tile uv corners from the uv_rects table), this pipeline takes an
// EXPLICIT per-vertex uv so the side walls can sample a single boundary texel's
// sub-tile uv from the block atlas.
//
// group(0) binding(0): a per-draw MVP mat4 via a dynamic-offset uniform (reuses
//   the model3d MVP buffer / 256-byte-slot pattern).
// group(1): the block atlas (texture + sampler), same shape as block.wgsl.
//
// Full-bright with a per-vertex directional `shade` (front/back/side) so the
// extrusion depth reads. Alpha-cutout (a < 0.5 discard). NO depth attachment
// (drawn in the hand pass over the world). Double-sided (cull off) so the back
// face and the inward-facing walls are never dropped.

struct MvpUniform {
    mvp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> m: MvpUniform;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:   vec3<f32>,
    @location(1) uv:    vec2<f32>,
    @location(2) shade: f32,
    @location(3) tint:  vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv:    vec2<f32>,
    @location(1) shade: f32,
    @location(2) tint:  vec3<f32>,
};

@vertex
fn vs_item(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = m.mvp * vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    out.tint = in.tint;
    return out;
}

@fragment
fn fs_item(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv);
    // Cutout: drop transparent texels so the stepped silhouette stays crisp.
    // 0.25, not 0.5: translucent block art (ice, ~0.49) must stay visible
    // on cutout-only presentation surfaces. See block.wgsl's fs_opaque.
    if (tex.a < 0.25) { discard; }
    // Full-bright * per-face directional shade * foliage tint (grass-green for a
    // held fern / short grass; white = no-op for flowers / tools / blocks).
    return vec4<f32>(tex.rgb * in.shade * in.tint, tex.a);
}
