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
    atlas_anim: vec4<u32>,
    // rgb = sim-owned sky light COLOUR (white = identity; night tints subtly
    // blue). Applied to the SKY term only — torch light keeps its warmth.
    sky_color: vec4<f32>,
    // xyz = unit sun direction, w = daylight [0,1] (atmosphere sun-glow).
    sun_dir: vec4<f32>,
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
    // Fragment − camera in render-local space: distance AND view direction for
    // the same atmosphere the world uses.
    @location(3) view:  vec3<f32>,
    // Absolute world height, for the atmosphere's altitude thinning.
    @location(4) world_y: f32,
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
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = in.pos.y;
    return out;
}

@fragment
fn fs_mob(in: VsOut) -> @location(0) vec4<f32> {
    let tex_color = textureSample(tex, samp, in.uv);
    // Cutout: drop transparent texels so the silhouette stays crisp and the
    // zero-area faces never paint stray pixels.
    if (tex_color.a < 0.5) { discard; }
    var color = tex_color.rgb * in.shade * in.tint;
    // Underwater: the same blue darkening multiply + tight linear murk the world
    // applies, so a submerged mob doesn't stay vividly lit against the murk.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), 1.0);
    }
    // The same atmosphere as the terrain, so a mob hazes out with the blocks
    // behind it.
    let out = atmosphere_apply(
        color,
        in.view,
        in.world_y,
        u.cam_pos.y + u.render_origin.y,
        u.fog.x,
        u.fog.y,
        u.fog_color.rgb,
        u.sun_dir.xyz,
        u.sun_dir.w,
    );
    return vec4<f32>(out, 1.0);
}

// --- world model blocks (the chunk `ModelVertex` stream) ---
//
// Same explicit-UV geometry as mobs, but lit at DRAW time: the vertex carries
// the cell's (sky, block) light separately and `shade` is only the directional
// face shade, so the sim's sky scale/colour darkens a placed model at night
// exactly like the terrain around it. The curve constants mirror block.wgsl —
// keep them in sync (at sky scale 1.0 + white sky the result is identical to
// the old mesh-time bake of max(sky, block)). The tint lane is PACKED here
// (unlike the mob vertex's three floats): a model block's colour is its
// texture, and the tint is a rare per-cell multiply on the cubes its row
// declared tintable.
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;
const SKY_GAMMA: f32 = 3.0;

struct WmIn {
    @location(0) pos:   vec3<f32>,
    @location(1) uv:    vec2<f32>,
    @location(2) shade: f32,
    // (sky, block_r, block_g, block_b) as four 6-bit levels packed
    // `sky | r<<6 | g<<12 | b<<18` (mesh::vertex::pack_model_light). They were
    // four floats; they are four integers scaled by 1/63, so the divide moved
    // here and the vertex lost 12 bytes of stride. The block channel is per
    // colour so a placed model sits in coloured light like the terrain.
    @location(3) light: u32,
    // Packed 0x00RRGGBB multiply colour; white unless the row declared this
    // cube tintable and the cell carries a tint.
    @location(4) tint: u32,
};

struct WmOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv:    vec2<f32>,
    @location(1) shade: f32,
    @location(2) view:  vec3<f32>,
    @location(3) light: vec4<f32>,
    @location(4) world_y: f32,
    @location(5) tint: vec3<f32>,
};

@vertex
fn vs_world_model(in: WmIn) -> WmOut {
    var out: WmOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = in.pos.y;
    out.light = vec4<f32>(
        f32(in.light & 63u),
        f32((in.light >> 6u) & 63u),
        f32((in.light >> 12u) & 63u),
        f32((in.light >> 18u) & 63u),
    ) / 63.0;
    out.tint = vec3<f32>(
        f32((in.tint >> 16u) & 255u),
        f32((in.tint >> 8u) & 255u),
        f32(in.tint & 255u),
    ) / 255.0;
    return out;
}

@fragment
fn fs_world_model(in: WmOut) -> @location(0) vec4<f32> {
    let tex_color = textureSample(tex, samp, in.uv);
    if (tex_color.a < 0.5) { discard; }
    // The same two-term light as block.wgsl: sky scaled + tinted by the sim's
    // day/night state, block light night-invariant and COLOURED, max of the two.
    let sky_term = mix(SKY_MIN, 1.0, pow(in.light.x, SKY_GAMMA) * u.fog_color.w) * u.sky_color.rgb;
    // Per channel, each riding its own SKY_MIN floor (see block.wgsl).
    let blk = in.light.yzw;
    let block_term = mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk);
    let lit = max(max(sky_term, block_term), vec3<f32>(FINAL_MIN));
    var color = tex_color.rgb * in.tint * in.shade * lit;
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), 1.0);
    }
    let out = atmosphere_apply(
        color,
        in.view,
        in.world_y,
        u.cam_pos.y + u.render_origin.y,
        u.fog.x,
        u.fog.y,
        u.fog_color.rgb,
        u.sun_dir.xyz,
        u.sun_dir.w,
    );
    return vec4<f32>(out, 1.0);
}

// The alpha-BLEND twin of fs_world_model for the chunk's semi-transparent model
// faces (the `model_blend_idx` stream): identical lighting, but the texture
// alpha survives to the blend unit instead of a 0.5 cutout. Only truly empty
// texels discard — a face reaches this stream because SOME texel in its rect
// has partial alpha, and the rest of the rect may still hold cutout holes.
@fragment
fn fs_world_model_blend(in: WmOut) -> @location(0) vec4<f32> {
    let tex_color = textureSample(tex, samp, in.uv);
    if (tex_color.a < 0.004) { discard; }
    let sky_term = mix(SKY_MIN, 1.0, pow(in.light.x, SKY_GAMMA) * u.fog_color.w) * u.sky_color.rgb;
    let blk = in.light.yzw;
    let block_term = mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk);
    let lit = max(max(sky_term, block_term), vec3<f32>(FINAL_MIN));
    var color = tex_color.rgb * in.tint * in.shade * lit;
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), tex_color.a);
    }
    let out = atmosphere_apply(
        color,
        in.view,
        in.world_y,
        u.cam_pos.y + u.render_origin.y,
        u.fog.x,
        u.fog.y,
        u.fog_color.rgb,
        u.sun_dir.xyz,
        u.sun_dir.w,
    );
    return vec4<f32>(out, tex_color.a);
}
