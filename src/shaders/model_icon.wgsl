// Inventory-icon shader for bbmodel blocks (the depthless UI pass).
//
// The held / dropped contexts draw the model with a depth buffer; the inventory slot
// is a DEPTHLESS pass, so `build_block_model_icon` instead bakes the icon's iso MVP into
// the vertex positions on the CPU and emits the faces PAINTER-SORTED (far→near). This
// stage is therefore a pure pass-through: positions arrive already in clip space, and
// the fragment samples the MODEL atlas (the block's own texture) with an alpha cutout,
// shaded + tinted like the in-world block — so the slot shows the real model, not a
// stand-in cube.

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) shade: f32,
    @location(3) tint: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) shade: f32,
    @location(2) tint: vec3<f32>,
};

@vertex
fn vs_model_icon(in: VsIn) -> VsOut {
    var out: VsOut;
    // Positions are PRE-TRANSFORMED to clip space on the CPU (the icon MVP is baked in),
    // so the vertex stage just passes them through.
    out.clip = vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    out.tint = in.tint;
    return out;
}

@group(0) @binding(0) var atlas_tex: texture_2d<f32>;
@group(0) @binding(1) var atlas_samp: sampler;

@fragment
fn fs_model_icon(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(atlas_tex, atlas_samp, in.uv);
    // Alpha cutout: transparent texels of the model texture leave the slot background.
    if (c.a < 0.5) {
        discard;
    }
    return vec4<f32>(c.rgb * in.shade * in.tint, 1.0);
}
