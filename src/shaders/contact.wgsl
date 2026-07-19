// contact: the model→terrain contact-shadow pass.
//
// Draws the chunk `ContactShadowVertex` stream — sparse non-indexed triangles
// coincident with the top face of the opaque cube under a bbmodel block's
// bottom footprint cells — with a MULTIPLY blend over the already-drawn opaque
// terrain (result = src.rgb * dst.rgb). The fragment outputs a grey
// `1 - darken`, so the stamp darkens the terrain under the model and white is
// the identity.
//
// Depth: LessEqual, read-only, with its own coplanar bias (the stamp shares the
// terrain face's plane). It draws BEFORE the sky pass on purpose: the stamp
// writes no depth, so if its supporting terrain section is culled while the
// model's section stays visible, the sky's far-plane LessEqual draw replaces
// the orphaned darkening with sky instead of leaving smudges on the background.
//
// Fog: the multiplier eases back to WHITE by the same atmosphere amount the
// terrain fogs with (linear murk underwater), reaching EXACT identity at the
// terminal fog distance — a darkening that survived into the haze would tint
// the fog and pop at the terrain cull boundary.

// Mirror of `render::uniforms::Uniforms`. Matches block.wgsl's layout.
struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog: vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    water_anim: vec4<u32>,
    sky_color: vec4<f32>,
    sun_dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct ContactIn {
    @location(0) pos:    vec3<f32>,
    @location(1) darken: f32,
};

struct ContactOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) darken:  f32,
    @location(1) view:    vec3<f32>,
    @location(2) world_y: f32,
};

@vertex
fn vs_contact(in: ContactIn) -> ContactOut {
    var out: ContactOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.darken = in.darken;
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = in.pos.y;
    return out;
}

@fragment
fn fs_contact(in: ContactOut) -> @location(0) vec4<f32> {
    let dist = length(in.view);
    var fade: f32;
    if (u.fog.w > 0.5) {
        // Underwater: the same tight linear murk band the world uses.
        fade = clamp((dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    } else {
        // atmosphere_amount() is exactly 1.0 at fog_end — the identity contract.
        fade = atmosphere_amount(dist, u.fog.x, u.fog.y, in.world_y, u.cam_pos.y + u.render_origin.y);
    }
    let m = mix(1.0 - in.darken, 1.0, fade);
    return vec4<f32>(m, m, m, 1.0);
}
