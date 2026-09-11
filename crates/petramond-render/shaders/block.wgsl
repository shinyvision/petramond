// Block vertex/fragment shader with atmosphere haze + directional face shading.
// Shared cel, atmosphere and water helpers are prepended by the pipeline.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>, // (start, end, time, underwater)
    // rgb = fog colour; w = sim-owned sky scale (1.0 = noon; mods dim it).
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    // Animated-water flipbook: (still_base_tile, flow_base_tile, frame_count, _).
    atlas_anim: vec4<u32>,
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

// Packed UV modes (bits 23..26; the UV_MODE_* constants are generated from
// the mesher's definitions):
// - dynamic thin geometry crops a 3/16-deep face to a matching strip instead of
//   squishing a whole 16px tile across a door edge.
// - CELL_LOCAL faces (stairs) carry an explicit tile-local UV in packed2 bits
//   6..11 / 11..16 (1/16ths), so a partial face samples the sub-rectangle of its
//   tile matching its position in the cell and the shape reads as a full block
//   with a chunk cut out.
// - modes from UV_MODE_TRANSITION up carry a texture-transition payload (see
//   texture_transition.wgsl); the mode's low bits are part of the set id.
const THIN_SLICE: f32 = 3.0 / 16.0;

// How the fragment stage composes a face's albedo (VsOut.overlay).
const FACE_PLAIN: u32 = 0u;
const FACE_OVERLAY: u32 = 1u;
const FACE_TRANSITION: u32 = 2u;

@group(0) @binding(0) var<uniform> u: Uniforms;
// The terrain pipeline samples a tile texture ARRAY: layer = tile id, uv is tile-LOCAL
// [0,1] (REPEAT-wrapped so a greedy-meshed quad can tile a single layer across a wide/tall
// face). group(0) binding 1 (the legacy `uv_rects` table) stays in the shared bind-group
// layout for the model/break/particle pipelines but is unused here.
@group(1) @binding(0) var atlas: texture_2d_array<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    // rgb = albedo tint; a = the block light's chroma low byte (block_light_rgb).
    @location(1) tint: vec4<f32>,
    // bits 0..11 = tile id, 11..13 = corner, 13..15 = shade index, 15..17 = AO,
    // 17..23 = SKYlight, 23..26 = UV mode, 26 = has-overlay,
    // 27..31 = block-light chroma high nibble, 31 = free.
    @location(2) packed: u32,
    // Second packed word: bits 0..6 = block light RED, 6..16 = cell-local uv
    // (CELL_LOCAL mode only), 16..19 = face normal code, 19 = dyed flag,
    // 20..31 = overlay payload (tile id / greedy span), 31 = free.
    @location(3) packed2: u32,
};

// Packed-column terrain: column-local XZ + world Y as i16 fixed-point (1/64
// block), plus the column's world XZ origin as an instance-step attribute.
struct VsInTerrain {
    // xyz = fixed-point pos; w = padding (wgpu has no i16x3 vertex format).
    @location(0) pos_q: vec4<i32>,
    @location(1) tint: vec4<f32>,
    @location(2) packed: u32,
    @location(3) packed2: u32,
    @location(4) col_origin: vec4<f32>,
};

const TERRAIN_POS_SCALE_INV: f32 = 1.0 / 64.0;

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
    @location(11) sky_exposure: f32,
    @location(12) @interpolate(flat) water: u32,
};

// Keep light hue and contact shading while softly grouping bright tones.
// The caller shares its view direction between the rim and atmosphere.
fn cel_shaded_light(in: VsOut, view_dir: vec3<f32>) -> vec3<f32> {
    var light = max(vec3<f32>(FINAL_MIN), in.light * cel_light_ratio(in.cel_drive));
    if (in.ncode != 0u) {
        light += cel_rim(face_normal(in.ncode), view_dir, light,
            u.sun_dir.xyz, in.sky_exposure * u.sun_dir.w);
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

// The vertex's BLOCK light, per channel in 0..1. RED rides packed2 bits 0..6;
// GREEN and BLUE ride a 12-bit chroma word split 8 + 4 between the tint's alpha
// lane and packed bits 27..31, each stored XOR the red channel — so colourless
// light writes no chroma bits at all and a white-lit vertex is bit-identical to
// the pre-colour engine's. Hand-mirrored from `mesh::vertex::BlockLight6`; the
// lane audit in `mesh::vertex` fails if the two drift.
fn block_light_rgb(packed: u32, packed2: u32, chroma_lo: f32) -> vec3<f32> {
    let r = packed2 & 0x3Fu;
    let chroma = u32(round(chroma_lo * 255.0)) | (((packed >> 27u) & 0xFu) << 8u);
    let g = (chroma & 0x3Fu) ^ r;
    let b = ((chroma >> 6u) & 0x3Fu) ^ r;
    return vec3<f32>(f32(r), f32(g), f32(b)) / 63.0;
}

fn vs_common(pos: vec3<f32>, tint: vec4<f32>, packed: u32, packed2: u32) -> VsOut {
    var out: VsOut;
    let local_pos = pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);

    let tile = packed & 0x7FFu;
    let corner = (packed >> 11u) & 0x3u;
    let overlay_tile = (packed2 >> 20u) & 0x7FFu;
    let ao = (packed >> 15u) & 0x3u;
    let sky6 = (packed >> 17u) & 0x3Fu;
    let uv_mode = (packed >> 23u) & 0x7u;
    let ncode = (packed2 >> 16u) & 0x7u;
    let transition = uv_mode >= UV_MODE_TRANSITION;
    // A transition face spends its shade lane on the set id; a cube face's
    // shade follows from its normal anyway.
    var shade_idx = (packed >> 13u) & 0x3u;
    if (transition) { shade_idx = face_shade_idx(ncode); }

    let atile = tile;

    // Tile-LOCAL uv in [0,1]; the array layer selects the tile.
    var uv = corner_local(corner);
    // The flow tile carries shader-side data in `overlay_tile` (no grass overlay
    // on water): top faces rotate toward the flow heading; side faces crop to the
    // water height. Still-water tops/bottoms are not the flow tile, so untouched.
    if (!transition && tile == u.atlas_anim.y) {
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
            var lh = pos.y - floor(pos.y);
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
        uv = cell_local_uv(packed2);
    } else {
        // uv_mode == NONE: plain cube face. A greedy-merged quad packs (W-1, H-1) into
        // packed2 bits 20..28 so its layer tiles W×H across the merge under the REPEAT sampler;
        // a normal 1×1 face has 0 there → ×(1,1), a no-op. Water tops/sides (flow
        // heading) and grass-side overlays reuse those bits for other data, so exclude
        // them (they are never greedy-merged by the mesher).
        let has_overlay = (packed >> 26u) & 0x1u;
        if (!transition && has_overlay == 0u && tile != u.atlas_anim.x && tile != u.atlas_anim.y) {
            let gw = f32(((packed2 >> 20u) & 0xFu) + 1u);
            let gh = f32(((packed2 >> 24u) & 0xFu) + 1u);
            uv = corner_local(corner) * vec2<f32>(gw, gh);
        }
    }
    out.uv = uv;
    // Dyed vertices (packed2 bit 19) sample the tile's dye-base twin: the
    // desaturated, brightness-normalized layers appended after the base set
    // (offset = tile count, carried in atlas_anim.w). The overlay layer
    // shifts with it so a dyed overlay-bearing face (a tinted grass side)
    // resolves WHOLLY in the dye-base domain — one primitive, no half-dyed
    // composite.
    let dyed_off = ((packed2 >> 19u) & 0x1u) * u.atlas_anim.w;
    out.layer = atile + dyed_off;
    out.water = select(0u, 1u, tile == u.atlas_anim.x || tile == u.atlas_anim.y);
    // Overlay uv: only grass sides (full cube faces) composite an overlay, so the
    // plain corner uv is always correct here.
    out.uv2 = corner_local(corner);
    out.overlay = select(FACE_PLAIN, FACE_OVERLAY, ((packed >> 26u) & 0x1u) == 1u);
    out.overlay_layer = overlay_tile + dyed_off;
    if (transition) {
        // The material grid rides the two layer lanes; the set id sits above
        // the ninth slot in the overlay lane.
        let data = transition_words(packed, packed2);
        out.layer = data.lo;
        out.overlay_layer = data.hi | (data.set_id << 4u);
        out.overlay = FACE_TRANSITION;
        out.water = 0u;
        out.uv = corner_local(corner);
    }

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
    //   - BLOCK term (torches, glowing plants): the SAME curve but night-invariant
    //     — no scale, no sky tint — so lit surfaces keep their day brightness at
    //     night. It is evaluated PER CHANNEL off the emitter's own colour, and
    //     each channel rides its own mix(SKY_MIN, ..) floor: a saturated purple's
    //     green channel bottoms out at the cave floor, never BELOW it (which would
    //     read as a black-green cast darker than an unlit cave). For colourless
    //     light the three channels are equal, so max(sky_term, block_term) == the
    //     old single-channel term of max(sky6, block6): the curve is monotone, so
    //     max commutes through it.
    //   - FINAL_MIN floors the darkest possible pixel: "very dark, not pitch black".
    var shades = array<f32, 4>(1.0, 0.85, 0.75, 0.55);
    var ao_lut = array<f32, 4>(0.25, 0.45, 0.70, 1.0);
    let sky = f32(sky6) / 63.0;
    let blk = block_light_rgb(packed, packed2, tint.a);
    let sky_curve = pow(sky, SKY_GAMMA);
    let sky_term = mix(SKY_MIN, 1.0, sky_curve * u.fog_color.w) * u.sky_color.rgb;
    // SKY_GAMMA is exactly 3: three multiplies, not three transcendental pows.
    let block_term = mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk);
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
    // Sky bounce colours contact shadows without lifting the cave floor.
    out.sky_exposure = sky;
    let occlusion = vec3<f32>(ao_lut[ao])
        + (1.0 - ao_lut[ao]) * vec3<f32>(0.04, 0.06, 0.09) * sky * u.sun_dir.w;
    out.light = max(
        vec3<f32>(FINAL_MIN),
        face_shade * occlusion * max(sky_term, block_term),
    );
    // Noon-equivalent light LEVEL (scale 1.0, white sky, no AO) for the cel
    // bands: the banding pattern must not slide around as the sim dims the sky
    // term, and AO stays a smooth multiplier so corners keep contact shadow.
    // Deliberately SCALAR — the block channel folds to its brightest component
    // first. A per-channel band steps each channel at a different threshold and
    // fringes every band edge with colour.
    let sky_noon = mix(SKY_MIN, 1.0, sky_curve);
    let blk_lum = max(blk.r, max(blk.g, blk.b));
    out.cel_drive = max(sky_noon, mix(SKY_MIN, 1.0, blk_lum * blk_lum * blk_lum));
    out.ncode = ncode;

    out.view = local_pos - u.cam_pos.xyz;
    out.tint = tint.rgb;
    out.world_pos = pos;
    return out;
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    return vs_common(in.pos, in.tint, in.packed, in.packed2);
}

@vertex
fn vs_terrain(in: VsInTerrain) -> VsOut {
    let pos = vec3<f32>(
        f32(in.pos_q.x) * TERRAIN_POS_SCALE_INV + in.col_origin.x,
        f32(in.pos_q.y) * TERRAIN_POS_SCALE_INV,
        f32(in.pos_q.z) * TERRAIN_POS_SCALE_INV + in.col_origin.z,
    );
    return vs_common(pos + greedy_overlap_push(in.packed, in.packed2), in.tint, in.packed, in.packed2);
}

// Sub-pixel tangent overlap for greedy-MERGED quads (1/1024 block). Their long
// edges meet per-cell neighbour faces as T-junctions, which rasterize one-pixel
// background cracks; a tiny outward push of each corner along the quad's own
// tangent plane covers them. It lives HERE, in f32, because the packed vertex
// grid is 1/64 block: baking the overlap either rounds it away (cracks return
// as bright speckles) or forces a full 1/64 skirt (visible wrap fringes +
// coplanar z-fighting). The gate mirrors the W×H tiling decode in vs_common:
// uv-mode NONE, no overlay, not a water tile — and a nonzero (W-1, H-1) field,
// so 1×1 faces (which never form T-junctions) stay mathematically exact.
fn greedy_overlap_push(packed: u32, packed2: u32) -> vec3<f32> {
    let uv_mode = (packed >> 23u) & 0x7u;
    let has_overlay = (packed >> 26u) & 0x1u;
    let tile = packed & 0x7FFu;
    let whf = (packed2 >> 20u) & 0xFFu;
    if (uv_mode != 0u || has_overlay == 1u || whf == 0u
        || tile == u.atlas_anim.x || tile == u.atlas_anim.y) {
        return vec3<f32>(0.0);
    }
    // corner_local -> {-1,+1} per tangent axis; du/dv map (u,v) quad space to
    // world axes per face (normal code 1..=6, Face::ALL order — the same
    // corner order as Face::quad_box).
    let c = corner_local((packed >> 11u) & 0x3u) * 2.0 - vec2<f32>(1.0, 1.0);
    var du = vec3<f32>(0.0);
    var dv = vec3<f32>(0.0);
    switch ((packed2 >> 16u) & 0x7u) {
        case NORMAL_POS_X: { du = vec3<f32>(0.0, 0.0, -1.0); dv = vec3<f32>(0.0, -1.0, 0.0); }
        case NORMAL_NEG_X: { du = vec3<f32>(0.0, 0.0, 1.0);  dv = vec3<f32>(0.0, -1.0, 0.0); }
        case NORMAL_POS_Y: { du = vec3<f32>(1.0, 0.0, 0.0);  dv = vec3<f32>(0.0, 0.0, 1.0); }
        case NORMAL_NEG_Y: { du = vec3<f32>(1.0, 0.0, 0.0);  dv = vec3<f32>(0.0, 0.0, -1.0); }
        case NORMAL_POS_Z: { du = vec3<f32>(1.0, 0.0, 0.0);  dv = vec3<f32>(0.0, -1.0, 0.0); }
        case NORMAL_NEG_Z: { du = vec3<f32>(-1.0, 0.0, 0.0); dv = vec3<f32>(0.0, -1.0, 0.0); }
        default: {}
    }
    return (du * c.x + dv * c.y) * (1.0 / 1024.0);
}

fn terrain_variant_layer(in: VsOut, layer: u32, donor: vec2<i32>) -> u32 {
    if (in.ncode == 0u || variation_count(layer, u.atlas_anim.w) <= 1u) { return layer; }
    let cell = variation_cell(in.view + u.cam_pos.xyz, vec3<i32>(u.render_origin.xyz), face_normal(in.ncode))
        + variation_donor_offset(donor, in.ncode);
    return block_variant_layer(layer, cell, u.atlas_anim.w);
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    // One view length/direction per fragment, shared by the rim, the
    // underwater murk, and the atmosphere.
    let dist = length(in.view);
    let vdir = in.view / max(dist, 1e-4);
    let grad_x = dpdx(in.uv);
    let grad_y = dpdy(in.uv);
    var rgb: vec3<f32>;
    if (in.overlay == FACE_TRANSITION) {
        rgb = transition_albedo(in, grad_x, grad_y);
    } else {
        let layer = terrain_variant_layer(in, in.layer, vec2<i32>(0));
        let base = sample_flipbook(layer, in.uv, grad_x, grad_y);
        if (in.overlay == FACE_OVERLAY) {
            // Grass side: untinted dirt base + biome-tinted grayscale grass overlay,
            // composited by the overlay's alpha so the grass matches the tinted top.
            // DYED faces (layer in the twin half, i.e. >= the tile count) tint the
            // base too: the whole face is in the dye-base domain, so the one
            // vertex tint (which folded the dye in at mesh time) applies uniformly.
            let ov = textureSample(atlas, samp, in.uv2, i32(in.overlay_layer));
            var b = base.rgb;
            if (in.layer >= u.atlas_anim.w) { b = b * in.tint; }
            rgb = mix(b, ov.rgb * in.tint, ov.a);
        } else {
            // Cutout: no asset authors texels in the 0.25..0.5 alpha band —
            // leaf fringes sit below 0.25, opaque art at 1.0, and TRANSLUCENT
            // art (ice, world-rendered in fs_transparent) at ~0.49, so item
            // cubes riding this pass draw it solid instead of vanishing.
            if (base.a < 0.25) { discard; }
            rgb = base.rgb * in.tint;
        }
    }
    var color = rgb * cel_shaded_light(in, vdir);
    // Underwater: blue darkening multiply + the tight linear murk fog.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), 1.0);
    }
    let out = atmosphere_apply_dir(
        color,
        dist,
        vdir,
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
    let dist = length(in.view);
    let vdir = in.view / max(dist, 1e-4);
    let tex = sample_flipbook(in.layer, in.uv, dpdx(in.uv), dpdy(in.uv));
    // Water has its own surface response; ice and glass keep authored alpha.
    if (tex.a < 0.03) { discard; }
    var albedo = tex.rgb;
    if (in.water != 0u) {
        // Water carries painted body colour as well as the flipbook detail;
        // multiplying two dark blues lets the brown lake bed dominate it.
        albedo = mix(albedo, vec3<f32>(0.32), 0.60);
    }
    var color = albedo * in.tint * cel_shaded_light(in, vdir);
    // Water blue tint + slight transparency.
    var alpha = select(tex.a, 0.78, in.water != 0u);
    // Tint the water volume itself when submerged so the surface seen from below
    // blends into the murk rather than glowing.
    if (u.fog.w > 0.5) {
        color = color * WATER_TINT;
        let f = clamp((dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
        return vec4<f32>(mix(color, u.fog_color.rgb, f), alpha);
    }
    if (in.water != 0u && in.ncode == NORMAL_POS_Y) {
        let surface = water_surface(
            color, in.view + u.cam_pos.xyz, u.render_origin.xyz,
            vdir, dist, u.fog.z, in.sky_exposure,
            u.fog_color.w, u.sky_color.rgb, u.fog_color.rgb, in.tint,
            u.sun_dir.xyz, u.sun_dir.w,
        );
        color = surface.color;
        alpha = surface.alpha;
    }
    let out = atmosphere_apply_dir(
        color,
        dist,
        vdir,
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

fn sample_flipbook(tile: u32, uv: vec2<f32>, grad_x: vec2<f32>, grad_y: vec2<f32>) -> vec4<f32> {
    let anim = tile_animation(tile);
    if (anim.x <= 1.0) {
        return textureSampleGrad(atlas, samp, uv, i32(tile), grad_x, grad_y);
    }
    let phase = u.fog.z * anim.y;
    let frame = u32(floor(phase)) % u32(anim.x);
    let color = textureSampleGrad(atlas, samp, uv, i32(tile + frame), grad_x, grad_y);
    if (anim.z == 0.0) { return color; }
    let next = textureSampleGrad(atlas, samp, uv, i32(tile + (frame + 1u) % u32(anim.x)), grad_x, grad_y);
    return mix(color, next, fract(phase));
}
