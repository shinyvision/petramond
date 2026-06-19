// Block vertex/fragment shader with fog + directional face shading.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, _, _)
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// uv-rect table: (u0, v0, u1, v1) per tile, baked on the CPU from tile_uv().
// The shader only SELECTS from it — no arithmetic — so uvs are bit-identical
// across backends. Size mirrors render::UV_RECTS_LEN.
@group(0) @binding(1) var<uniform> uv_rects: array<vec4<f32>, 16>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    @location(1) tint: vec3<f32>,
    // bits 0..8 = tile id, 8..10 = corner (0..3), 10..12 = shade index.
    @location(2) packed: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light: f32,
    @location(2) dist: f32,
    @location(3) tint: vec3<f32>,
    @location(4) uv2: vec2<f32>,
    @location(5) @interpolate(flat) overlay: u32,
};

// Select a tile-rect corner. r = (u0,v0,u1,v1); corner order matches the mesher:
// 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
fn corner_uv(r: vec4<f32>, corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(r.x, r.w); }
    if (corner == 1u) { return vec2<f32>(r.z, r.w); }
    if (corner == 2u) { return vec2<f32>(r.z, r.y); }
    return vec2<f32>(r.x, r.y);
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(in.pos, 1.0);

    let tile = in.packed & 0xFFu;
    let corner = (in.packed >> 8u) & 0x3u;
    let shade_idx = (in.packed >> 10u) & 0x3u;
    let overlay_tile = (in.packed >> 12u) & 0xFFu;

    out.uv = corner_uv(uv_rects[tile], corner);
    out.uv2 = corner_uv(uv_rects[overlay_tile], corner);
    out.overlay = (in.packed >> 20u) & 0x1u;

    // Mirror of mesh::SHADES — keep byte-identical.
    var shades = array<f32, 4>(1.0, 0.85, 0.75, 0.55);
    out.light = shades[shade_idx];

    out.dist = length(u.cam_pos.xyz - in.pos);
    out.tint = in.tint;
    return out;
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(atlas, samp, in.uv);
    var rgb: vec3<f32>;
    if (in.overlay == 1u) {
        // Grass side: untinted dirt base + biome-tinted grayscale grass overlay,
        // composited by the overlay's alpha so the grass matches the tinted top.
        let ov = textureSample(atlas, samp, in.uv2);
        rgb = mix(base.rgb, ov.rgb * in.tint, ov.a);
    } else {
        if (base.a < 0.5) { discard; } // leaf/cutout
        rgb = base.rgb * in.tint;
    }
    let color = rgb * in.light;
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(color, u.fog_color.rgb, f);
    return vec4<f32>(out, 1.0);
}

@fragment
fn fs_transparent(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv);
    // Only water uses this alpha-blended pass now (leaves render fully opaque in
    // fs_opaque). Water tiles are full-alpha, so the discard is a no-op for them.
    if (tex.a < 0.5) { discard; }
    let color = tex.rgb * in.tint * in.light;
    // Water blue tint + slight transparency.
    let alpha = 0.78;
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(vec3<f32>(color), u.fog_color.rgb, f);
    return vec4<f32>(out, alpha);
}