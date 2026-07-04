// Block vertex/fragment shader with fog + directional face shading.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, time, underwater)
    // rgb = fog colour; w = sim-owned sky scale (1.0 = noon; mods dim it).
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    // Animated-water flipbook: (still_base_tile, flow_base_tile, frame_count, _).
    water_anim: vec4<u32>,
    // rgb = sim-owned sky light COLOUR (white = identity; mods tint the night
    // subtly blue). Applied to the SKY term only — torch light keeps its warmth.
    sky_color: vec4<f32>,
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

// Packed UV modes (bits 29..32):
// - dynamic thin geometry crops a 3/16-deep face to a matching strip instead of
//   squishing a whole 16px tile across a door edge.
// - stair side/top faces sample from their position inside the block cell, so the
//   lower half-step and full-height back half read as one continuous block.
const THIN_SLICE: f32 = 3.0 / 16.0;
const UV_MODE_THIN_U: u32 = 1u;
const UV_MODE_THIN_V: u32 = 2u;
const UV_MODE_STAIR_POS_X: u32 = 3u;
const UV_MODE_STAIR_NEG_X: u32 = 4u;
const UV_MODE_STAIR_POS_Z: u32 = 5u;
const UV_MODE_STAIR_NEG_Z: u32 = 6u;
const UV_MODE_STAIR_TOP: u32 = 7u;

@group(0) @binding(0) var<uniform> u: Uniforms;
// The terrain pipeline samples a tile texture ARRAY: layer = tile id, uv is tile-LOCAL
// [0,1] (REPEAT-wrapped so a greedy-meshed quad can tile a single layer across a wide/tall
// face). group(0) binding 1 (the legacy `uv_rects` table) stays in the shared bind-group
// layout for the model/break/particle pipelines but is unused here.
@group(1) @binding(0) var atlas: texture_2d_array<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    @location(1) tint: vec3<f32>,
    // bits 0..8 = tile id, 8..10 = corner, 10..12 = shade index,
    // 12..20 = overlay tile, 20 = has-overlay, 21..23 = AO, 23..29 = SKYlight,
    // 29..32 = UV mode.
    @location(2) packed: u32,
    // Second packed word: bits 0..6 = block (torch) light, rest reserved (zero).
    @location(3) packed2: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Per-channel light: the sky term is tinted by the sim's sky colour, the
    // block term is not, so the two can differ per channel at night.
    @location(1) light: vec3<f32>,
    @location(2) dist: f32,
    @location(3) tint: vec3<f32>,
    @location(4) uv2: vec2<f32>,
    @location(5) @interpolate(flat) overlay: u32,
    @location(6) world_pos: vec3<f32>,
    // Texture-array layers (tile ids): the base tile and the overlay tile. Flat.
    @location(7) @interpolate(flat) layer: u32,
    @location(8) @interpolate(flat) overlay_layer: u32,
};

// Per-corner UV in unit-tile space [0,1]^2. Corner order matches the mesher:
// 0->(0,1) 1->(1,1) 2->(1,0) 3->(0,0). This IS the tile-local sample coord now that
// every tile is its own array layer (no atlas sub-rect remap).
fn corner_local(corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(0.0, 1.0); }
    if (corner == 1u) { return vec2<f32>(1.0, 1.0); }
    if (corner == 2u) { return vec2<f32>(1.0, 0.0); }
    return vec2<f32>(0.0, 0.0);
}

fn cell_axis_coord(axis: f32, upper: bool) -> f32 {
    var v = axis - floor(axis);
    if (upper && v < 0.001) { v = 1.0; }
    return v;
}

fn stair_side_uv(corner: u32, pos: vec3<f32>, mode: u32) -> vec2<f32> {
    let right = corner == 1u || corner == 2u;
    let top = corner == 2u || corner == 3u;
    let y = cell_axis_coord(pos.y, top);
    var u01 = 0.0;
    if (mode == UV_MODE_STAIR_POS_X) {
        let z = cell_axis_coord(pos.z, !right);
        u01 = 1.0 - z;
    } else if (mode == UV_MODE_STAIR_NEG_X) {
        let z = cell_axis_coord(pos.z, right);
        u01 = z;
    } else if (mode == UV_MODE_STAIR_POS_Z) {
        let x = cell_axis_coord(pos.x, right);
        u01 = x;
    } else {
        let x = cell_axis_coord(pos.x, !right);
        u01 = 1.0 - x;
    }
    return vec2<f32>(u01, 1.0 - y);
}

fn stair_top_uv(corner: u32, pos: vec3<f32>) -> vec2<f32> {
    let right = corner == 1u || corner == 2u;
    let far_z = corner == 0u || corner == 1u;
    let x = cell_axis_coord(pos.x, right);
    let z = cell_axis_coord(pos.z, far_z);
    return vec2<f32>(x, z);
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);

    let tile = in.packed & 0xFFu;
    let corner = (in.packed >> 8u) & 0x3u;
    let shade_idx = (in.packed >> 10u) & 0x3u;
    let overlay_tile = (in.packed >> 12u) & 0xFFu;
    let ao = (in.packed >> 21u) & 0x3u;
    let sky6 = (in.packed >> 23u) & 0x3Fu;
    let uv_mode = (in.packed >> 29u) & 0x7u;

    // Animate water: WaterStill / WaterFlow are the first of `frame_count`
    // consecutive flipbook tiles; advance base + frame over time.
    var atile = tile;
    let frames = u.water_anim.z;
    if (frames > 0u && (tile == u.water_anim.x || tile == u.water_anim.y)) {
        var fps = WATER_STILL_FPS;
        if (tile == u.water_anim.y) { fps = WATER_FLOW_FPS; }
        atile = tile + (u32(floor(u.fog.z * fps)) % frames);
    }

    // Tile-LOCAL uv in [0,1]; the array layer selects the tile.
    var uv = corner_local(corner);
    // The flow tile carries shader-side data in `overlay_tile` (no grass overlay
    // on water): top faces rotate toward the flow heading; side faces crop to the
    // water height. Still-water tops/bottoms are not the flow tile, so untouched.
    if (tile == u.water_anim.y) {
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
            uv = vec2<f32>(rel.x * cs - rel.y * sn, rel.x * sn + rel.y * cs) * inv
                + vec2<f32>(0.5, 0.5);
        } else if (shade_idx == 1u || shade_idx == 2u) {
            // SIDE: map the tile's V to this vertex's height within its cell, so a
            // partial sheet (thin flow) or a trimmed exposed step shows the matching
            // slice of the texture instead of squishing/stretching the full tile.
            // v=0 at the cell top, v=1 at the bottom. A full-height top vertex lands on
            // an integer Y (fract 0), so treat that as height 1.
            var lh = in.pos.y - floor(in.pos.y);
            if ((corner == 2u || corner == 3u) && lh < 0.001) { lh = 1.0; }
            uv.y = 1.0 - lh;
        }
    }
    // UV modes above the base packed face corner. Doors crop thin edge faces;
    // stairs remap side/top faces to their cell-local X/Z/Y coordinates.
    if (uv_mode == UV_MODE_THIN_U || uv_mode == UV_MODE_THIN_V) {
        if (uv_mode == UV_MODE_THIN_U) {
            let lu = select(0.0, 1.0, corner == 1u || corner == 2u);
            uv.x = lu * THIN_SLICE;
        } else {
            let lv = select(0.0, 1.0, corner == 0u || corner == 1u);
            uv.y = lv * THIN_SLICE;
        }
    } else if (uv_mode >= UV_MODE_STAIR_POS_X && uv_mode <= UV_MODE_STAIR_NEG_Z) {
        uv = stair_side_uv(corner, in.pos, uv_mode);
    } else if (uv_mode == UV_MODE_STAIR_TOP) {
        uv = stair_top_uv(corner, in.pos);
    } else {
        // uv_mode == NONE: plain cube face. A greedy-merged quad packs (W-1, H-1) into
        // bits 12..20 so its layer tiles W×H across the merge under the REPEAT sampler;
        // a normal 1×1 face has 0 there → ×(1,1), a no-op. Water tops/sides (flow
        // heading) and grass-side overlays reuse those bits for other data, so exclude
        // them (they are never greedy-merged by the mesher).
        let has_overlay = (in.packed >> 20u) & 0x1u;
        if (has_overlay == 0u && tile != u.water_anim.x && tile != u.water_anim.y) {
            let gw = f32(((in.packed >> 12u) & 0xFu) + 1u);
            let gh = f32(((in.packed >> 16u) & 0xFu) + 1u);
            uv = corner_local(corner) * vec2<f32>(gw, gh);
        }
    }
    out.uv = uv;
    out.layer = atile;
    var uv2 = corner_local(corner);
    if (uv_mode >= UV_MODE_STAIR_POS_X && uv_mode <= UV_MODE_STAIR_NEG_Z) {
        uv2 = stair_side_uv(corner, in.pos, uv_mode);
    } else if (uv_mode == UV_MODE_STAIR_TOP) {
        uv2 = stair_top_uv(corner, in.pos);
    }
    out.uv2 = uv2;
    out.overlay = (in.packed >> 20u) & 0x1u;
    out.overlay_layer = overlay_tile;

    // Final vertex light = directional face shade (mirror of mesh::SHADES — keep
    // byte-identical) * per-vertex AO * max(sky term, block term), all smoothly
    // interpolated so shadows and the light-level gradient are soft.
    //   - AO_LUT: contact-shadow dip in lit areas.
    //   - each 6-bit channel: 0..63 -> 0..1; a gamma curve keeps near-full light
    //     bright while mid/low levels fall off, mixed up from SKY_MIN so a fully
    //     occluded face never goes black.
    //   - SKY term: scaled by fog_color.w (the sim's sky scale) INSIDE the mix so
    //     scale 1.0 is exactly identity and scale 0 bottoms out at the SKY_MIN cave
    //     floor; tinted by sky_color.rgb (white = identity).
    //   - BLOCK term (torches/furnaces): the SAME curve but night-invariant — no
    //     scale, no tint — so torchlit surfaces keep their day brightness at night.
    //     At scale 1.0 + white sky, max(sky_term, block_term) == the old single-
    //     channel term of max(sky6, block6): the curve is monotone, so max commutes
    //     through it. (The warm HUE still rides the CPU-baked tint.)
    //   - FINAL_MIN floors the darkest possible pixel: "very dark, not pitch black".
    var shades = array<f32, 4>(1.0, 0.85, 0.75, 0.55);
    var ao_lut = array<f32, 4>(0.25, 0.45, 0.70, 1.0);
    let block6 = in.packed2 & 0x3Fu;
    let sky = f32(sky6) / 63.0;
    let blk = f32(block6) / 63.0;
    let sky_term = mix(SKY_MIN, 1.0, pow(sky, SKY_GAMMA) * u.fog_color.w) * u.sky_color.rgb;
    let block_term = mix(SKY_MIN, 1.0, pow(blk, SKY_GAMMA));
    out.light = max(
        vec3<f32>(FINAL_MIN),
        shades[shade_idx] * ao_lut[ao] * max(sky_term, vec3<f32>(block_term)),
    );

    out.dist = length(u.cam_pos.xyz - local_pos);
    out.tint = in.tint;
    out.world_pos = in.pos;
    return out;
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(atlas, samp, in.uv, i32(in.layer));
    var rgb: vec3<f32>;
    if (in.overlay == 1u) {
        // Grass side: untinted dirt base + biome-tinted grayscale grass overlay,
        // composited by the overlay's alpha so the grass matches the tinted top.
        let ov = textureSample(atlas, samp, in.uv2, i32(in.overlay_layer));
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
    let tex = textureSample(atlas, samp, in.uv, i32(in.layer));
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
