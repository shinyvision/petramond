// Block vertex/fragment shader with atmosphere haze + directional face shading.
// pipeline.rs prepends atmosphere.wgsl (the shared haze model) to this source.

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
    // xyz = unit sun direction, w = daylight [0,1] (atmosphere sun-glow).
    sun_dir: vec4<f32>,
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
// - CELL_LOCAL faces (stairs) carry an explicit tile-local UV in packed2 bits
//   6..11 / 11..16 (1/16ths), so a partial face samples the sub-rectangle of its
//   tile matching its position in the cell and the shape reads as a full block
//   with a chunk cut out.
const THIN_SLICE: f32 = 3.0 / 16.0;
const UV_MODE_THIN_U: u32 = 1u;
const UV_MODE_THIN_V: u32 = 2u;
const UV_MODE_CELL_LOCAL: u32 = 3u;

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
    // Second packed word: bits 0..6 = block (torch) light, 6..16 = cell-local uv
    // (CELL_LOCAL mode only), rest reserved (zero).
    @location(3) packed2: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Per-channel light: the sky term is tinted by the sim's sky colour, the
    // block term is not, so the two can differ per channel at night.
    @location(1) light: vec3<f32>,
    // Fragment − camera in render-local space (unnormalized): distance AND view
    // direction for the atmosphere in one interpolant.
    @location(2) view: vec3<f32>,
    @location(3) tint: vec3<f32>,
    @location(4) uv2: vec2<f32>,
    @location(5) @interpolate(flat) overlay: u32,
    @location(6) world_pos: vec3<f32>,
    // Texture-array layers (tile ids): the base tile and the overlay tile. Flat.
    @location(7) @interpolate(flat) layer: u32,
    @location(8) @interpolate(flat) overlay_layer: u32,
    // Face normal code (packed2 bits 16..19) for the fragment-side cel rim.
    @location(9) @interpolate(flat) ncode: u32,
    // Day-invariant light LEVEL (max(sky, block) at noon scale + white sky, no
    // AO): drives the fragment-side cel banding so the bands stay put while the
    // sim's day/night scale, the tint, and the smooth AO ride on `light`
    // untouched.
    @location(10) cel_drive: f32,
};

// Cel-banded fragment light (cel.wgsl): quantize the interpolated light by the
// banded/raw ratio of the day-invariant drive (hue, night dimming, and AO
// survive), faded to identity in the dark and applied at partial strength so
// cave gradients stay gradual; then add the view-angle rim on faces that carry
// a normal.
fn cel_shaded_light(in: VsOut) -> vec3<f32> {
    let banded = cel_band(CEL_LIGHT, in.cel_drive);
    let f = smoothstep(CEL_LIGHT_FADE_LO, CEL_LIGHT_FADE_HI, in.cel_drive)
        * CEL_LIGHT_STRENGTH;
    let cel = mix(1.0, banded / max(in.cel_drive, 1e-4), f);
    var light = max(vec3<f32>(FINAL_MIN), in.light * cel);
    if (CEL_RIM_ENABLED && in.ncode != 0u) {
        light += cel_rim(face_normal(in.ncode), normalize(in.view), light);
    }
    return light;
}

// Per-corner UV in unit-tile space [0,1]^2. Corner order matches the mesher:
// 0->(0,1) 1->(1,1) 2->(1,0) 3->(0,0). This IS the tile-local sample coord now that
// every tile is its own array layer (no atlas sub-rect remap).
fn corner_local(corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(0.0, 1.0); }
    if (corner == 1u) { return vec2<f32>(1.0, 1.0); }
    if (corner == 2u) { return vec2<f32>(1.0, 0.0); }
    return vec2<f32>(0.0, 0.0);
}

// Explicit tile-local UV carried in packed2 bits 6..11 (u) / 11..16 (v), in
// 1/16ths of a tile. Read only for UV_MODE_CELL_LOCAL vertices.
fn cell_local_uv(packed2: u32) -> vec2<f32> {
    return vec2<f32>(
        f32((packed2 >> 6u) & 0x1Fu),
        f32((packed2 >> 11u) & 0x1Fu),
    ) / 16.0;
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
    // stairs carry explicit cell-local UVs.
    if (uv_mode == UV_MODE_THIN_U || uv_mode == UV_MODE_THIN_V) {
        if (uv_mode == UV_MODE_THIN_U) {
            let lu = select(0.0, 1.0, corner == 1u || corner == 2u);
            uv.x = lu * THIN_SLICE;
        } else {
            let lv = select(0.0, 1.0, corner == 0u || corner == 1u);
            uv.y = lv * THIN_SLICE;
        }
    } else if (uv_mode == UV_MODE_CELL_LOCAL) {
        uv = cell_local_uv(in.packed2);
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
    // Overlay uv: only grass sides (full cube faces) composite an overlay, so the
    // plain corner uv is always correct here.
    out.uv2 = corner_local(corner);
    out.overlay = (in.packed >> 20u) & 0x1u;
    out.overlay_layer = overlay_tile;

    // Final vertex light = directional face shade * per-vertex AO *
    // max(sky term, block term), all smoothly interpolated so shadows and the
    // light-level gradient are soft.
    //   - Face shade: SUN-DIRECTIONAL for terrain faces carrying a normal code
    //     (packed2 bits 16..19): N·L against the moving sun, warm on lit faces,
    //     cool in shadow — the flat storybook look. Code
    //     0 (cross plants, torches, dynamic props) keeps the classic SHADES
    //     table (mirror of mesh::SHADES — keep byte-identical).
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
    let sky_curve = pow(sky, SKY_GAMMA);
    let sky_term = mix(SKY_MIN, 1.0, sky_curve * u.fog_color.w) * u.sky_color.rgb;
    let block_term = mix(SKY_MIN, 1.0, pow(blk, SKY_GAMMA));
    let ncode = (in.packed2 >> 16u) & 0x7u;
    var face_shade = vec3<f32>(shades[shade_idx]);
    if (ncode != 0u) {
        // Sun colours only where the sky actually reaches. The warm-lit /
        // cool-shadow split is daylight bathing the face; underground it has no
        // light source, and it read as yellow cave ceilings next to gray cave
        // walls. Fade the sun ramp out with the vertex's sky light and rest on
        // the neutral shade table in the dark.
        let sun_shade = sun_face_shade(face_normal(ncode), u.sun_dir.xyz, u.sun_dir.w);
        face_shade = mix(face_shade, sun_shade, sky);
    }
    out.light = max(
        vec3<f32>(FINAL_MIN),
        face_shade * ao_lut[ao] * max(sky_term, vec3<f32>(block_term)),
    );
    // Noon-equivalent light level (scale 1.0, white sky, no AO) for the cel
    // bands: the banding pattern must not slide around as the sim dims the sky
    // term, and AO stays a smooth multiplier so corners keep contact shadow.
    let sky_noon = mix(SKY_MIN, 1.0, sky_curve);
    out.cel_drive = max(sky_noon, block_term);
    out.ncode = ncode;

    out.view = local_pos - u.cam_pos.xyz;
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
        // Cutout: no asset authors texels in the 0.25..0.5 alpha band —
        // leaf fringes sit below 0.25, opaque art at 1.0, and TRANSLUCENT
        // art (ice, world-rendered in fs_transparent) at ~0.49, so item
        // cubes riding this pass draw it solid instead of vanishing.
        if (base.a < 0.25) { discard; }
        rgb = base.rgb * in.tint;
    }
    var color = rgb * cel_shaded_light(in);
    // Underwater: blue darkening multiply + the tight linear murk fog.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), 1.0);
    }
    let out = atmosphere_apply(
        color,
        in.view,
        in.world_pos.y,
        u.cam_pos.y + u.render_origin.y,
        u.fog.x,
        u.fog.y,
        u.fog_color.rgb,
        u.sun_dir.xyz,
        u.sun_dir.w,
    );
    return vec4<f32>(out, 1.0);
}

@fragment
fn fs_transparent(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, samp, in.uv, i32(in.layer));
    // Two tenants share this alpha-blended pass, split by authored alpha:
    // water tiles are full-alpha and take the water constant below, while a
    // TRANSLUCENT block tile (ice) is authored under the opaque pass's 0.5
    // cutout and keeps its own texture alpha. Only near-zero texels discard.
    if (tex.a < 0.03) { discard; }
    var color = tex.rgb * in.tint * cel_shaded_light(in);
    // Water blue tint + slight transparency.
    let alpha = select(tex.a, 0.78, tex.a >= 0.5);
    // Tint the water volume itself when submerged so the surface seen from below
    // blends into the murk rather than glowing.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((length(in.view) - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), alpha);
    }
    let out = atmosphere_apply(
        color,
        in.view,
        in.world_pos.y,
        u.cam_pos.y + u.render_origin.y,
        u.fog.x,
        u.fog.y,
        u.fog_color.rgb,
        u.sun_dir.xyz,
        u.sun_dir.w,
    );
    return vec4<f32>(out, alpha);
}
