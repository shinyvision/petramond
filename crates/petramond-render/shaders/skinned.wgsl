// skinned: GPU-skinned animated bodies (mobs, player bodies).
//
// Concatenated AFTER mob.wgsl (it reuses `u`, the entity texture at
// group(1), `VsOut`, `fs_mob` and the light-curve constants declared there;
// the frame `Uniforms` struct itself comes from the shared prelude, imported
// here too so this file names its own dependency). Each model's cubes live in a static bind-space vertex buffer (see
// render::skinned::SkinMesh); the per-frame CPU work is only the skeleton:
// a bone palette of `G · pose[bone]` matrices at group(2) and one instance
// row per body (tint, sampled light, palette base, hidden parts). The vertex
// shader applies the palette entry and lights the body per instance with the
// same two-channel curve block.wgsl and render::lighting use, so the output
// matches the old CPU bake that folded all of this into world-space vertices.

#import petramond::frame

@group(2) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;

struct SkinIn {
    // Per vertex (static mesh).
    @location(0) pos:   vec3<f32>,
    @location(1) uv:    vec2<f32>,
    @location(2) shade: f32,
    @location(3) bone:  u32,
    @location(4) parts: u32,
    // Per instance.
    @location(5) tint:      vec3<f32>,
    @location(6) self_lit:  f32,
    // (sky, block r, g, b) as fractions of full light.
    @location(7) light:     vec4<f32>,
    @location(8) bone_base: u32,
    @location(9) hidden:    u32,
};

// render::lighting::light_rgb folded toward full bright by `self_lit`
// (render::lighting::fold_self_lit): the sky term scaled by the sim's sky
// scale and tinted by its colour, the block term per channel on its own
// SKY_MIN floor, max of the two, floored at FINAL_MIN.
fn body_light(light: vec4<f32>, self_lit: f32) -> vec3<f32> {
    let x = light.x;
    let sky = SKY_MIN + (1.0 - SKY_MIN) * (x * x * x * clamp(u.fog_color.w, 0.0, 1.0));
    let sky_term = vec3<f32>(sky) * u.sky_color.rgb;
    let blk = light.yzw;
    let block_term = mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk);
    let lit = max(max(sky_term, block_term), vec3<f32>(FINAL_MIN));
    return lit + (vec3<f32>(1.0) - lit) * self_lit;
}

@vertex
fn vs_skinned(in: SkinIn) -> VsOut {
    var out: VsOut;
    // A hidden part (a shorn coat) collapses every vertex of its cubes onto
    // one point, so its triangles rasterize nothing.
    if ((in.parts & in.hidden) != 0u) {
        out.clip = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        return out;
    }
    let local_pos = (bones[in.bone_base + in.bone] * vec4<f32>(in.pos, 1.0)).xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    out.tint = in.tint * body_light(in.light, in.self_lit);
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = local_pos.y + f32(u.render_origin.y);
    return out;
}
