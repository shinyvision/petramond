// particles: tiny 3D cubes. Mining/break particles are textured cutout cubes;
// block-row emitters use the transparent solid-color fragment entry below.
//
// Cubes are built CPU-side (world-space, 6 faces each) with a compact per-vertex
// format: pos + ABSOLUTE atlas uv + RGB tint + per-face shade + alpha. group(0) is
// the shared Uniforms + uv_rects bind (the SAME bind group the block pipeline uses
// — uv_rects is unused here but declared so the layout matches and the bind is
// reused). group(1) is the block atlas. The fragment samples the absolute uv,
// multiplies by shade (per-face directional shading so the cube reads 3D) and tint
// (foliage-green for grass/leaf flecks, white otherwise). An alpha CUTOUT
// (discard a<0.5) keeps the cubes solid and depth-WRITING so they are correctly
// occluded by terrain and visible from any angle including above. End-of-life fade
// is done CPU-side by SHRINKING the cube; alpha gates the cutout.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    water_anim: vec4<u32>,
    sky_color: vec4<f32>,
    // xyz = unit sun direction, w = daylight [0,1] (atmosphere sun-glow).
    sun_dir: vec4<f32>,
};

// Underwater multiply tint — kept in sync with block.wgsl so dust submerged with
// the player reads the same blue darkening as the terrain around it.
const WATER_TINT: vec3<f32> = vec3<f32>(0.42, 0.62, 0.85);

@group(0) @binding(0) var<uniform> u: Uniforms;
// Unused by particles (uv is absolute, per-vertex) but declared so this pipeline
// can reuse the block pipeline's `uniform_bind` bind group unchanged.
@group(0) @binding(1) var<uniform> uv_rects: array<vec4<f32>, 256>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:   vec3<f32>,
    @location(1) uv:    vec2<f32>,
    @location(2) tint:  vec3<f32>,
    @location(3) shade: f32,
    @location(4) alpha: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) shade: f32,
    @location(3) alpha: f32,
    // Fragment − camera in render-local space: distance AND view direction for
    // the same atmosphere the world uses.
    @location(4) view: vec3<f32>,
    // Absolute world height, for the atmosphere's altitude thinning.
    @location(5) world_y: f32,
};

@vertex
fn vs_particle(in: VsIn) -> VsOut {
    var out: VsOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.uv = in.uv;
    out.tint = in.tint;
    out.shade = in.shade;
    out.alpha = in.alpha;
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = in.pos.y;
    return out;
}

@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv);
    // Alpha cutout: gate on the atlas alpha AND the particle's fade alpha so a
    // nearly-faded cube cuts out. Depth-WRITING (set in the pipeline) keeps the
    // solid cubes correctly occluded and self-sorting.
    let a = tex.a * in.alpha;
    // 0.25, not 0.5: ice break-burst texels (~0.49 alpha) must survive.
    if (a < 0.25) { discard; }
    // shade = per-face directional shading; tint multiplies the atlas colour
    // (white = no change; foliage-green greens a grass/leaf fleck).
    var color = tex.rgb * in.tint * in.shade;
    // Submerged with the player: blue darkening + tight murk fog to match the
    // underwater terrain; in air, the same atmosphere as the terrain, so a break
    // burst hazes out with the surrounding blocks instead of staying crisp.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), 1.0);
    }
    color = atmosphere_apply(
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
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_particle_transparent(in: VsOut) -> @location(0) vec4<f32> {
    var color = in.tint * in.shade;
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), clamp(in.alpha, 0.0, 1.0));
    }
    color = atmosphere_apply(
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
    return vec4<f32>(color, clamp(in.alpha, 0.0, 1.0));
}
