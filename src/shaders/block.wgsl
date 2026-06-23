// Block vertex/fragment shader with fog + directional face shading.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, time, underwater)
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    // Animated-water flipbook: (still_base_tile, flow_base_tile, frame_count, _).
    water_anim: vec4<u32>,
};

// Flipbook playback speed (frames/second) for still vs flowing water. Flowing
// water reads a touch faster so it visibly streams.
const WATER_STILL_FPS: f32 = 8.0;
const WATER_FLOW_FPS: f32 = 12.0;

// Skylight floor: a fully sky-occluded surface fades to this fraction of its lit
// value rather than to black. FINAL_MIN is the absolute darkest pixel ("very
// dark, not pitch black"). Kept low so an unlit cave is genuinely dark and a
// torch's block-light (folded into the same channel) reads dramatically against
// it. Keep in sync with `model3d.wgsl` and `render::lighting`.
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;
// Steepness of the light->dark falloff: higher = more of the range reads dark.
const SKY_GAMMA: f32 = 3.0;

// Underwater look: a multiply tint (darker + blue) applied to everything seen
// while submerged.
const WATER_TINT: vec3<f32> = vec3<f32>(0.42, 0.62, 0.85);

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

// Per-corner UV in unit-tile space [0,1]^2 (same corner order as `corner_uv`),
// used to rotate the flowing-water tile about its centre.
fn corner_local(corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(0.0, 1.0); }
    if (corner == 1u) { return vec2<f32>(1.0, 1.0); }
    if (corner == 2u) { return vec2<f32>(1.0, 0.0); }
    return vec2<f32>(0.0, 0.0);
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

    // Animate water: WaterStill / WaterFlow are the first of `frame_count`
    // consecutive flipbook tiles; advance base + frame over time.
    var atile = tile;
    let frames = u.water_anim.z;
    if (frames > 0u && (tile == u.water_anim.x || tile == u.water_anim.y)) {
        var fps = WATER_STILL_FPS;
        if (tile == u.water_anim.y) { fps = WATER_FLOW_FPS; }
        atile = tile + (u32(floor(u.fog.z * fps)) % frames);
    }

    var uv = corner_uv(uv_rects[atile], corner);
    // The flow tile carries shader-side data in `overlay_tile` (no grass overlay
    // on water): top faces rotate toward the flow heading; side faces crop to the
    // water height. Still-water tops/bottoms are not the flow tile, so untouched.
    if (tile == u.water_anim.y) {
        let r = uv_rects[atile];
        if (shade_idx == 0u) {
            // TOP: rotate the tile about its centre by the flow heading so a cell
            // streaming into a corner points diagonally, not snapped to a cardinal.
            // Scale by 1/(|cos|+|sin|) so the rotated square stays inscribed in the
            // tile (no bleed into neighbours); cardinals are unscaled.
            let a = (f32(overlay_tile) / 256.0 - 0.5) * 6.2831853;
            let rel = corner_local(corner) - vec2<f32>(0.5, 0.5);
            let cs = cos(a);
            let sn = sin(a);
            let inv = 1.0 / (abs(cs) + abs(sn));
            let rr = vec2<f32>(rel.x * cs - rel.y * sn, rel.x * sn + rel.y * cs) * inv
                + vec2<f32>(0.5, 0.5);
            uv = vec2<f32>(r.x, r.y) + rr * vec2<f32>(r.z - r.x, r.w - r.y);
        } else if (shade_idx == 1u || shade_idx == 2u) {
            // SIDE: map the tile's V to this vertex's height within its cell, so a
            // partial sheet (thin flow) or a trimmed exposed step shows the matching
            // slice of the texture instead of squishing/stretching the full tile.
            // v0 at the cell top, v1 at the bottom. A full-height top vertex lands on
            // an integer Y (fract 0), so treat that as height 1.
            var lh = in.pos.y - floor(in.pos.y);
            if ((corner == 2u || corner == 3u) && lh < 0.001) { lh = 1.0; }
            uv.y = r.y + (1.0 - lh) * (r.w - r.y);
        }
    }
    out.uv = uv;
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
    // Underwater: blue darkening multiply.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
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
