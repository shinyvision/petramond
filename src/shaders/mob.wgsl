// mob: in-world animated entity models (the owl).
//
// Draws CPU-baked, world-space, skeletally-posed geometry (see
// render::owl_model::build_owls) with the shared world `view_proj` at group(0)
// and a DEDICATED entity texture (NOT the block atlas) at group(1). Vertices are
// the explicit-per-vertex `ItemVertex` (pos, uv, shade, tint) used by the item3d
// pipeline, so the model's arbitrary sub-rectangle UVs sample the entity sheet
// directly (the model3d packed-vertex shader can only SELECT whole-tile corners).
//
// Full lighting is baked into `shade` on the CPU (face directional shade × the
// instance's sampled world skylight), matching item_model. Alpha-cutout so the
// texture's transparent texels (and zero-area faces of flat sub-cubes) drop out;
// depth-tested + written in its own world pass so the owl occludes and is occluded
// by terrain. Double-sided (the pipeline disables back-face culling) so the owl's
// flat 2D sub-cubes — legs, tail — show from both sides.

// Mirror of `render::uniforms::Uniforms`. Matches block.wgsl's layout.
struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog: vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    water_anim: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

// Underwater look: the same multiply tint (darker + blue) the world shader applies, so
// a submerged mob murks out with the terrain around it. Keep in sync with block.wgsl.
const WATER_TINT: vec3<f32> = vec3<f32>(0.42, 0.62, 0.85);

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
    // Distance from the camera, for the same distance fog the world uses.
    @location(3) dist:  f32,
};

@vertex
fn vs_mob(in: VsIn) -> VsOut {
    var out: VsOut;
    // Positions are baked in world space on the CPU; subtract the current render
    // origin so the GPU transform stays camera-local far from spawn.
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    out.tint = in.tint;
    out.dist = length(u.cam_pos.xyz - local_pos);
    return out;
}

@fragment
fn fs_mob(in: VsOut) -> @location(0) vec4<f32> {
    let tex_color = textureSample(tex, samp, in.uv);
    // Cutout: drop transparent texels so the silhouette stays crisp and the
    // zero-area faces never paint stray pixels.
    if (tex_color.a < 0.5) { discard; }
    var color = tex_color.rgb * in.shade * in.tint;
    // Underwater: the same blue darkening multiply the world applies, so a submerged
    // mob doesn't stay vividly lit against the murk.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
    }
    // Distance fog (underwater swaps in a short, murky-blue fog on the CPU), so a mob
    // fades into the fog exactly like the blocks behind it.
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(color, u.fog_color.rgb, f);
    return vec4<f32>(out, 1.0);
}
