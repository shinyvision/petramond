// model3d: first-person hand + isometric inventory icons.
//
// Draws small full-bright models (textured block cubes, solid-color skin hand, or
// flat sprite billboards) using a per-draw MVP matrix supplied via a
// dynamic-offset uniform at group(0) binding(0). group(1) is the block atlas
// (texture + sampler), same shape as block.wgsl's atlas_bind.
//
// Vertex format is the shared 24-byte mesh::Vertex (see block.wgsl's VsIn). The
// `packed` word folds tile / corner / shade / solid-flag / AO / skylight plus
// the block light's chroma high nibble; this shader reconstructs the uv
// (SELECTING from the uv_rects table — never recomputing) and the face shade,
// exactly like the chunk pipeline, so a held block is textured identically to
// the world block.
//
// The overlay flag (vtx_overlay_flag) is overloaded (mirrors block_model's
// packing):
//  - solid cuboid (flag set, NO tile / NO overlay): the tint IS the colour
//    tile (both 0) -> output the interpolated vertex `tint` directly.
//  - grass-block side: flag set + a real overlay tile (vtx_overlay_payload) ->
//    sample the dirt base + tinted grayscale grass-side overlay and composite
//    (exactly like block.wgsl::fs_opaque), so out-of-world grass sides green to
//    match the top.
//  - flag clear: sample the atlas tile * face shade * tint (leaves/grass-top
//    foliage tint, or untinted blocks/flowers).
// The two set-bit cases are told apart by whether the overlay tile is non-zero
// (the solid path packs no tiles at all).

struct MvpUniform {
    mvp: mat4x4<f32>,
};

// The frame `Uniforms` — only fog_color.w (the sim's sky scale) and
// sky_color.rgb are read here, so the held block dims/tints in step with
// terrain. The icon-atlas bake binds the same buffer at its init values
// (w = 1.0, sky_color = white), so icons stay full-bright. The uv-rect table
// (u0,v0,u1,v1 per tile, SELECT only) and the vertex lane decoders are the
// same modules block.wgsl reads.
#import petramond::frame
#import petramond::uv_rects
#import petramond::vertex

// Keep in sync with `block.wgsl` / `render::lighting` (dark cave floor).
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;
const SKY_GAMMA: f32 = 3.0;

// UV_MODE_CELL_LOCAL: the vertex carries an explicit tile-local UV
// (vtx_cell_uv). Stair item cubes use it so their partial faces sample the
// matching sub-rectangle of the tile instead of restarting it per quad. Modes
// from UV_MODE_TRANSITION up are terrain texture-transition payloads; this
// pipeline draws its own geometry (item cubes and bbmodels), which never emits
// them, so any other mode reads as a plain face.

@group(0) @binding(0) var<uniform> m: MvpUniform;
@group(0) @binding(2) var<uniform> frame: Uniforms;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    // rgb = albedo tint; a = the block light's chroma low byte (block_light_rgb).
    @location(1) tint: vec4<f32>,
    // The two packed words, decoded through petramond::vertex (only the
    // CELL_LOCAL UV mode is honoured here; the normal code is unused).
    @location(2) packed: u32,
    @location(3) packed2: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) light: vec3<f32>,
    // Overlay flag (set for solid hand OR grass-side overlay).
    @location(3) @interpolate(flat) flag: u32,
    // overlay (grass-side) uv, sampled when `overlay_tile` is non-zero.
    @location(4) uv2: vec2<f32>,
    // overlay tile id (0 = none); disambiguates the solid vs overlay flag use.
    @location(5) @interpolate(flat) overlay_tile: u32,
    // Half-texel-inset sample bounds of the base / overlay tile (see inset_tile).
    @location(6) @interpolate(flat) uv_bounds: vec4<f32>,
    @location(7) @interpolate(flat) uv2_bounds: vec4<f32>,
};

// Shrink a tile rect (u0,v0,u1,v1) toward its centre by half a texel on every
// edge. One tile is 16 texels wide/tall, so a half-texel is (rect_span)*(0.5/16).
// World blocks (block.wgsl) are large and never sample exactly at a tile edge, so
// they need no inset. But the small, rotated, magnified iso icons + held hand
// (this shader) DO interpolate uv right at the tile boundary, and with a full-tile
// rect those edge fragments bleed into the ADJACENT atlas tile (a 1px sliver of a
// wrong texture down the centre seam where the cube faces meet).
//
// The guard is applied as a per-FRAGMENT clamp into this inset rect — NOT by
// reconstructing the corner uvs from the inset rect. Corner insetting stretches
// the inner 15 texels of the tile across the full quad, so a 16px sprite icon
// baked at 64px renders 15 uneven ~4.27px texels (and its edge rows half-clipped)
// instead of 16 crisp 4px ones. Clamping leaves every interior fragment's uv
// untouched (texel-exact icons) and only pulls true edge fragments inside the
// tile, which lands on the same edge texel under nearest sampling.
fn inset_tile(r: vec4<f32>) -> vec4<f32> {
    let inset = (vec2<f32>(r.z - r.x, r.w - r.y)) * (0.5 / 16.0);
    return vec4<f32>(r.x + inset.x, r.y + inset.y, r.z - inset.x, r.w - inset.y);
}

// Clamp an interpolated uv into its tile's inset bounds (u0,v0,u1,v1).
fn clamp_uv(uv: vec2<f32>, b: vec4<f32>) -> vec2<f32> {
    return clamp(uv, b.xy, b.zw);
}

// Same corner mapping as block.wgsl: 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
fn corner_unit(corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(0.0, 1.0); }
    if (corner == 1u) { return vec2<f32>(1.0, 1.0); }
    if (corner == 2u) { return vec2<f32>(1.0, 0.0); }
    return vec2<f32>(0.0, 0.0);
}

fn corner_uv(r: vec4<f32>, corner: u32) -> vec2<f32> {
    return mix(r.xy, r.zw, corner_unit(corner));
}

// A row-declared UV quarter turn (0..3), the WGSL twin of `ShapeFace::turn_uv`
// (mirror of block.wgsl's) — applied to a plain cube face's unit uv BEFORE it
// is lerped into the tile rect, so a held/dropped/icon cube turns exactly like
// the placed block.
fn turn_uv(turn: u32, uv: vec2<f32>) -> vec2<f32> {
    if (turn == 1u) { return vec2<f32>(uv.y, 1.0 - uv.x); }
    if (turn == 2u) { return vec2<f32>(1.0 - uv.x, 1.0 - uv.y); }
    if (turn == 3u) { return vec2<f32>(1.0 - uv.y, uv.x); }
    return uv;
}

@vertex
fn vs_model(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = m.mvp * vec4<f32>(in.pos, 1.0);

    let tile = vtx_tile(in.packed);
    let corner = vtx_corner(in.packed);
    let shade_idx = vtx_shade(in.packed);
    let overlay_tile = vtx_overlay_payload(in.packed2);
    let ao = vtx_ao(in.packed);
    let sky6 = vtx_sky(in.packed);
    let uv_mode = vtx_uv_mode(in.packed);

    // Reconstruct uvs from the FULL tile rect (texel-exact mapping); the
    // fragment stage clamps them into the half-texel-inset bounds so edge
    // fragments of the magnified iso icons never sample the neighbour tile
    // (see inset_tile). Applied to BOTH the base uv and the grass-side overlay uv2.
    // Dyed vertices (vtx_dyed) shift the tile rect into the dye-base half of
    // the composed atlas (twins sit exactly half the texture down).
    let dye_v = f32(vtx_dyed(in.packed2)) * 0.5;
    var r = uv_rects[tile];
    r = vec4<f32>(r.x, r.y + dye_v, r.z, r.w + dye_v);
    out.uv_bounds = inset_tile(r);
    if (uv_mode == UV_MODE_CELL_LOCAL) {
        out.uv = mix(r.xy, r.zw, vtx_cell_uv(in.packed2));
    } else {
        // The row's UV quarter turn (see mesh::vertex::pack_uv_turn);
        // CELL_LOCAL faces bake their mapping into the explicit uv above, so
        // only plain faces turn here.
        let uv_turn = vtx_uv_turn(in.packed, in.packed2);
        out.uv = mix(r.xy, r.zw, turn_uv(uv_turn, corner_unit(corner)));
    }
    let r2 = uv_rects[overlay_tile];
    out.uv2 = corner_uv(r2, corner);
    out.uv2_bounds = inset_tile(r2);
    // Mirror of block.wgsl lighting: directional shade * AO * max(sky term, block
    // term), with the same sky-scale/-colour lanes so the held block dims and
    // tints with the world (identity at scale 1.0 + white) while torch light stays
    // night-invariant. See block.wgsl for the identity argument.
    var shades = array<f32, 4>(1.0, 0.85, 0.75, 0.55);
    var ao_lut = array<f32, 4>(0.25, 0.45, 0.70, 1.0);
    let sky = f32(sky6) / 63.0;
    let blk = block_light_rgb(in.packed, in.packed2, in.tint.a);
    let sky_term =
        mix(SKY_MIN, 1.0, pow(sky, SKY_GAMMA) * frame.fog_color.w) * frame.sky_color.rgb;
    // Per channel, each riding its own SKY_MIN floor (see block.wgsl).
    let block_term = mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk);
    out.light = max(
        vec3<f32>(FINAL_MIN),
        shades[shade_idx] * ao_lut[ao] * max(sky_term, block_term),
    );
    out.tint = in.tint.rgb;
    out.flag = vtx_overlay_flag(in.packed);
    out.overlay_tile = overlay_tile;
    return out;
}

@fragment
fn fs_model(in: VsOut) -> @location(0) vec4<f32> {
    if (in.flag == 1u && in.overlay_tile != 0u) {
        // Grass-block side: untinted dirt base + biome-tinted grayscale grass-side
        // overlay, composited by the overlay's alpha (mirror of block.wgsl
        // fs_opaque) so the side greens to match the tinted top.
        let base = textureSample(atlas, samp, clamp_uv(in.uv, in.uv_bounds));
        let ov = textureSample(atlas, samp, clamp_uv(in.uv2, in.uv2_bounds));
        let rgb = mix(base.rgb, ov.rgb * in.tint, ov.a);
        return vec4<f32>(rgb * in.light, 1.0);
    }
    if (in.flag == 1u) {
        // Skin hand / solid cuboid: output the vertex color, shaded per face so
        // the cuboid reads as a 3D shape. Fully opaque.
        return vec4<f32>(in.tint * in.light, 1.0);
    }
    let tex = textureSample(atlas, samp, clamp_uv(in.uv, in.uv_bounds));
    // Flat sprite items (flowers) and leaf cutouts: drop transparent texels so the
    // billboard cuts out cleanly. Block cubes are full-alpha so this is a no-op.
    // 0.25, not 0.5 — matches the other cutout passes (see block.wgsl).
    if (tex.a < 0.25) { discard; }
    return vec4<f32>(tex.rgb * in.tint * in.light, tex.a);
}
