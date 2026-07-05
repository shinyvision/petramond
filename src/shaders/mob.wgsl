// mob: in-world animated entity models.
//
// Draws CPU-baked, world-space, skeletally-posed geometry (see
// render::mob_model::build_mob_instances) with the shared world `view_proj` at group(0)
// and a DEDICATED entity texture (NOT the block atlas) at group(1). Vertices are
// the explicit-per-vertex `ItemVertex` (pos, uv, shade, tint) used by the item3d
// pipeline, so the model's arbitrary sub-rectangle UVs sample the entity sheet
// directly (the model3d packed-vertex shader can only SELECT whole-tile corners).
//
// Full lighting is baked into `shade` on the CPU (face directional shade × the
// instance's sampled world skylight), matching item_model. Alpha-cutout so the
// texture's transparent texels (and zero-area faces of flat sub-cubes) drop out;
// depth-tested + written in its own world pass so mobs occlude and are occluded
// by terrain. Double-sided (the pipeline disables back-face culling) so flat
// sub-cubes such as legs and tails show from both sides.

// Mirror of `render::uniforms::Uniforms`. Matches block.wgsl's layout.
struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog: vec4<f32>,
    // rgb = fog colour; w = sim-owned sky scale (1.0 = noon; night dims it).
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    water_anim: vec4<u32>,
    // rgb = sim-owned sky light COLOUR (white = identity; night tints subtly
    // blue). Applied to the SKY term only — torch light keeps its warmth.
    sky_color: vec4<f32>,
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

// --- world model blocks (the chunk `ModelVertex` stream) ---
//
// Same explicit-UV geometry as mobs, but lit at DRAW time: the vertex carries
// the cell's (sky, block) light separately and `shade` is only the directional
// face shade, so the sim's sky scale/colour darkens a placed model at night
// exactly like the terrain around it. The curve constants mirror block.wgsl —
// keep them in sync (at sky scale 1.0 + white sky the result is identical to
// the old mesh-time bake of max(sky, block)).
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;
const SKY_GAMMA: f32 = 3.0;

struct WmIn {
    @location(0) pos:   vec3<f32>,
    @location(1) uv:    vec2<f32>,
    @location(2) shade: f32,
    @location(3) tint:  vec3<f32>,
    @location(4) light: vec2<f32>, // (sky01, block01)
};

struct WmOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv:    vec2<f32>,
    @location(1) shade: f32,
    @location(2) tint:  vec3<f32>,
    @location(3) dist:  f32,
    @location(4) light: vec2<f32>,
};

@vertex
fn vs_world_model(in: WmIn) -> WmOut {
    var out: WmOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    out.tint = in.tint;
    out.dist = length(u.cam_pos.xyz - local_pos);
    out.light = in.light;
    return out;
}

@fragment
fn fs_world_model(in: WmOut) -> @location(0) vec4<f32> {
    let tex_color = textureSample(tex, samp, in.uv);
    if (tex_color.a < 0.5) { discard; }
    // The same two-term light as block.wgsl: sky scaled + tinted by the sim's
    // day/night state, block light night-invariant, max of the two.
    let sky_term = mix(SKY_MIN, 1.0, pow(in.light.x, SKY_GAMMA) * u.fog_color.w) * u.sky_color.rgb;
    let block_term = mix(SKY_MIN, 1.0, pow(in.light.y, SKY_GAMMA));
    let lit = max(max(sky_term, vec3<f32>(block_term)), vec3<f32>(FINAL_MIN));
    var color = tex_color.rgb * in.shade * in.tint * lit;
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
    }
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(color, u.fog_color.rgb, f);
    return vec4<f32>(out, 1.0);
}
