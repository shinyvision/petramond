// particles: tiny 3D cubes. Mining/break particles are textured cutout cubes;
// block-row emitters use the transparent solid-color fragment entry below.
//
// Each particle is ONE instance row (render::particles::ParticleRow: centre,
// half size, stretch, ABSOLUTE atlas uv rect, lit RGB tint, alpha, or an
// oriented quad's axes); the vertex stage expands it into the cube's 24
// vertices from `vertex_index` (face = index / 4, corner = index % 4) and the
// face table render::particles::wgsl_faces prepends. group(0) is the shared
// Uniforms + uv_rects bind (the SAME bind group the block pipeline uses —
// uv_rects is unused here but declared so the layout matches and the bind is
// reused). group(1) is the block atlas. The fragment samples the absolute uv,
// multiplies by shade (per-face directional shading so the cube reads 3D) and tint
// (foliage-green for grass/leaf flecks, white otherwise). An alpha CUTOUT
// (discard a<0.5) keeps the cubes solid and depth-WRITING so they are correctly
// occluded by terrain and visible from any angle including above. End-of-life fade
// is done CPU-side by SHRINKING the cube (its row's half size); alpha gates the
// cutout.

#import petramond::frame

@group(0) @binding(0) var<uniform> u: Uniforms;
// The uv-rect table is unused by particles (uv is absolute, per-vertex) but
// imported so this pipeline can reuse the block pipeline's `uniform_bind`
// bind group unchanged.
#import petramond::uv_rects
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct ParticleIn {
    @builtin(vertex_index) vertex: u32,
    @location(0) center:  vec3<f32>,
    @location(1) half_size: f32,
    @location(2) right:   vec3<f32>,
    @location(3) stretch: f32,
    @location(4) up:      vec3<f32>,
    @location(5) alpha:   f32,
    @location(6) uv_min:  vec2<f32>,
    @location(7) uv_max:  vec2<f32>,
    @location(8) tint:    vec3<f32>,
    @location(9) quad:    u32,
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
fn vs_particle(in: ParticleIn) -> VsOut {
    var out: VsOut;
    let face = in.vertex / 4u;
    let corner = in.vertex % 4u;
    // Corners bl, br, tr, tl — the order the uv corners follow, v growing
    // downward in the atlas like the block pipeline.
    let sx = select(-1.0, 1.0, corner == 1u || corner == 2u);
    let sy = select(-1.0, 1.0, corner >= 2u);
    var pos: vec3<f32>;
    var shade: f32;
    if (in.quad != 0u) {
        // An oriented quad is the first face alone; the other five collapse
        // onto one point and rasterize nothing.
        if (face != 0u) {
            out.clip = vec4<f32>(0.0, 0.0, 0.0, 1.0);
            return out;
        }
        pos = in.center + in.right * sx + in.up * sy;
        shade = 1.0;
    } else {
        let right = particle_face_right[face];
        let up = particle_face_up[face];
        // The face plane sits on the cube SURFACE, offset outward along its
        // normal (right x up points out), not through the centre.
        let fc = in.center + cross(right, up) * in.half_size;
        pos = fc + right * in.half_size * sx + up * in.half_size * sy;
        pos.y = in.center.y + (pos.y - in.center.y) * in.stretch;
        shade = particle_face_shade[face];
    }
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.uv = vec2<f32>(
        select(in.uv_min.x, in.uv_max.x, corner == 1u || corner == 2u),
        select(in.uv_max.y, in.uv_min.y, corner >= 2u),
    );
    out.tint = in.tint;
    out.shade = shade;
    out.alpha = in.alpha;
    out.view = pos - u.cam_pos.xyz;
    out.world_y = pos.y + f32(u.render_origin.y);
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
    // Inside a fluid with the player: the eye medium's tint + murk fog to match the
    // murky terrain; in air, the same atmosphere as the terrain, so a break
    // burst hazes out with the surrounding blocks instead of staying crisp.
    if (u.fog.w > 0.5) {
        color = color * u.volume_tint.rgb;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), 1.0);
    }
    color = atmosphere_apply(
        color,
        in.view,
        in.world_y,
        u.cam_pos.y + f32(u.render_origin.y),
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
        color = color * u.volume_tint.rgb;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), clamp(in.alpha, 0.0, 1.0));
    }
    color = atmosphere_apply(
        color,
        in.view,
        in.world_y,
        u.cam_pos.y + f32(u.render_origin.y),
        u.fog.x,
        u.fog.y,
        u.fog_color.rgb,
        u.sun_dir.xyz,
        u.sun_dir.w,
    );
    return vec4<f32>(color, clamp(in.alpha, 0.0, 1.0));
}
