// entity shadow: the blob-shadow decals under mobs / dropped items / bodies.
//
// Draws one horizontal quad per entity — the presentation gather resolves the
// ground height, scales the quad to the entity's footprint, and fades the
// strength with how high the body sits above its ground. MULTIPLY blend over
// the already-drawn opaque terrain (result = src.rgb * dst.rgb): the fragment
// outputs grey `1 - darken` with a radial falloff from the corner offset, so
// white is the identity and overlapping stamps compose naturally.
//
// Depth: LessEqual, read-only, with the contact-stamp's coplanar bias (the
// quad shares the ground face's plane, exactly like the model contact stamps).
// It draws BEFORE the sky pass on purpose: the quad writes no depth, so if its
// supporting terrain section is culled, the sky's far-plane LessEqual draw
// replaces the orphaned darkening with sky instead of leaving smudges on the
// background.
//
// Fog: the multiplier eases back to WHITE by the same atmosphere amount the
// terrain fogs with (linear murk underwater), reaching EXACT identity at the
// terminal fog distance — a shadow that survived into the haze would tint the
// fog and pop at the terrain cull boundary.

// Mirror of `render::uniforms::Uniforms`. Matches contact.wgsl's layout.
struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog: vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    atlas_anim: vec4<u32>,
    sky_color: vec4<f32>,
    sun_dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// Fraction of the quad radius that is fully dark before the soft edge begins.
const CORE: f32 = 0.35;

struct ShadowIn {
    @location(0) pos:      vec3<f32>,
    @location(1) corner:   vec2<f32>,
    @location(2) strength: f32,
};

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) strength: f32,
    @location(1) corner:   vec2<f32>,
    @location(2) view:     vec3<f32>,
    @location(3) world_y:  f32,
};

@vertex
fn vs_entity_shadow(in: ShadowIn) -> ShadowOut {
    var out: ShadowOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.strength = in.strength;
    out.corner = in.corner;
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = in.pos.y;
    return out;
}

@fragment
fn fs_entity_shadow(in: ShadowOut) -> @location(0) vec4<f32> {
    let d = length(in.corner);
    // Solid core, smooth falloff to the quad edge.
    let fall = 1.0 - smoothstep(CORE, 1.0, d);
    var m = 1.0 - in.strength * fall;
    let dist = length(in.view);
    var fade: f32;
    if (u.fog.w > 0.5) {
        // Underwater: the same tight linear murk band the world uses.
        fade = clamp((dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    } else {
        // atmosphere_amount() is exactly 1.0 at fog_end — the identity contract.
        fade = atmosphere_amount(dist, u.fog.x, u.fog.y, in.world_y, u.cam_pos.y + u.render_origin.y);
    }
    m = mix(m, 1.0, fade);
    return vec4<f32>(m, m, m, 1.0);
}
