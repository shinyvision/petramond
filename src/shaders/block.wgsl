// Block vertex/fragment shader with fog + directional face shading.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, time, underwater)
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};

// Skylight floor: a fully sky-occluded surface fades to this fraction of its lit
// value rather than to black. FINAL_MIN is the absolute darkest pixel ("very
// dark, not pitch black"). Tune these to taste.
const SKY_MIN: f32 = 0.05;
const FINAL_MIN: f32 = 0.02;
// Steepness of the light->dark falloff: higher = more of the range reads dark.
const SKY_GAMMA: f32 = 3.0;

// Underwater look: a multiply tint (darker + blue) applied to everything seen
// while submerged, plus animated caustics added on lit surfaces. The caustic is
// cheap layered sine "ridges" drifting over time, sampled in world space so the
// dappled light is continuous across block boundaries.
const WATER_TINT: vec3<f32> = vec3<f32>(0.42, 0.62, 0.85);
const CAUSTIC_COLOR: vec3<f32> = vec3<f32>(0.45, 0.85, 1.0);
// Caustics are deliberately *barely* noticeable: just a faint shimmer on lit
// surfaces, not a bold dappled pattern.
const CAUSTIC_STRENGTH: f32 = 0.12;
const CAUSTIC_SCALE: f32 = 0.55;

@group(0) @binding(0) var<uniform> u: Uniforms;
// uv-rect table: (u0, v0, u1, v1) per tile, baked on the CPU from tile_uv().
// The shader only SELECTS from it — no arithmetic — so uvs are bit-identical
// across backends. Size MUST mirror render::UV_RECTS_LEN (= 256, the 8-bit tile
// cap); a mismatch fails `packed_vertex_pipeline_validates`.
@group(0) @binding(1) var<uniform> uv_rects: array<vec4<f32>, 256>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    @location(1) tint: vec3<f32>,
    // bits 0..8 = tile id, 8..10 = corner, 10..12 = shade index,
    // 12..20 = overlay tile, 20 = has-overlay, 21..23 = AO, 23..29 = skylight.
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
    @location(6) world_pos: vec3<f32>,
};

// Select a tile-rect corner. r = (u0,v0,u1,v1); corner order matches the mesher:
// 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
fn corner_uv(r: vec4<f32>, corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(r.x, r.w); }
    if (corner == 1u) { return vec2<f32>(r.z, r.w); }
    if (corner == 2u) { return vec2<f32>(r.z, r.y); }
    return vec2<f32>(r.x, r.y);
}

// Animated caustic ridges in [0,1]: layered sines combined into thin bright
// filaments, sharpened by a cubic. p is a world-space xz position; t is seconds.
fn caustic(p: vec2<f32>, t: f32) -> f32 {
    let q = p * CAUSTIC_SCALE;
    let a = sin(q.x + t * 1.3) + sin(q.y - t * 1.1);
    let b = sin((q.x + q.y) * 0.7 + t * 1.7) + sin((q.x - q.y) * 0.9 - t * 1.5);
    let v = (a + b) * 0.25 + 0.5;
    let s = clamp(v, 0.0, 1.0);
    return pow(s, 3.0);
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(in.pos, 1.0);

    let tile = in.packed & 0xFFu;
    let corner = (in.packed >> 8u) & 0x3u;
    let shade_idx = (in.packed >> 10u) & 0x3u;
    let overlay_tile = (in.packed >> 12u) & 0xFFu;
    let ao = (in.packed >> 21u) & 0x3u;
    let sky6 = (in.packed >> 23u) & 0x3Fu;

    out.uv = corner_uv(uv_rects[tile], corner);
    out.uv2 = corner_uv(uv_rects[overlay_tile], corner);
    out.overlay = (in.packed >> 20u) & 0x1u;

    // Final vertex light = directional face shade (mirror of mesh::SHADES — keep
    // byte-identical) * per-vertex AO * per-vertex skylight, all smoothly
    // interpolated so shadows and the light-level gradient are soft.
    //   - AO_LUT: contact-shadow dip in lit areas.
    //   - skylight: 0..63 -> 0..1; a gamma curve keeps near-full sky bright while
    //     mid/low levels fall off, mixed up from SKY_MIN so a sky-occluded face
    //     never goes black.
    //   - FINAL_MIN floors the darkest possible pixel: "very dark, not pitch black".
    var shades = array<f32, 4>(1.0, 0.85, 0.75, 0.55);
    var ao_lut = array<f32, 4>(0.25, 0.45, 0.70, 1.0);
    let sky = f32(sky6) / 63.0;
    let sky_term = mix(SKY_MIN, 1.0, pow(sky, SKY_GAMMA));
    out.light = max(FINAL_MIN, shades[shade_idx] * ao_lut[ao] * sky_term);

    out.dist = length(u.cam_pos.xyz - in.pos);
    out.tint = in.tint;
    out.world_pos = in.pos;
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
    var color = rgb * in.light;
    // Underwater: blue darkening + animated caustics. The caustic is scaled by the
    // surface's own light so shadowed/deep areas stay dark.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let c = caustic(in.world_pos.xz, u.fog.z) * in.light;
        color = color + CAUSTIC_COLOR * (c * CAUSTIC_STRENGTH);
    }
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(color, u.fog_color.rgb, f);
    return vec4<f32>(out, 1.0);
}

@fragment
fn fs_transparent(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv);
    // Only water uses this alpha-blended pass; water tiles are full-alpha so the
    // discard is a no-op for them.
    if (tex.a < 0.5) { discard; }
    var color = tex.rgb * in.tint * in.light;
    // Tint the water volume itself when submerged so the surface seen from below
    // blends into the murk rather than glowing.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
    }
    // Water blue tint + slight transparency.
    let alpha = 0.78;
    let f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    let out = mix(color, u.fog_color.rgb, f);
    return vec4<f32>(out, alpha);
}
