// model3d: first-person hand + isometric inventory icons.
//
// Draws small full-bright models (textured block cubes, solid-color skin hand, or
// flat sprite billboards) using a per-draw MVP matrix supplied via a
// dynamic-offset uniform at group(0) binding(0). group(1) is the block atlas
// (texture + sampler), same shape as block.wgsl's atlas_bind.
//
// Vertex format is the shared 28-byte mesh::Vertex (see block.wgsl's VsIn). The
// `packed` word folds tile / corner / shade / solid-flag / AO / skylight; this
// shader reconstructs the uv (SELECTING from the uv_rects table — never
// recomputing) and the face shade, exactly like the chunk pipeline, so a held
// block is textured identically to the world block.
//
// Bit 20 is overloaded (mirrors block_model's packing):
//  - solid cuboid (skin hand, SOLID_COLOR_FLAG): bit 20 set, NO tile / NO overlay
//    tile (both 0) -> output the interpolated vertex `tint` directly.
//  - grass-block side: bit 20 set + a real overlay tile in bits 12..20 -> sample
//    the dirt base + tinted grayscale grass-side overlay and composite (exactly
//    like block.wgsl::fs_opaque), so out-of-world grass sides green to match the
//    top.
//  - bit 20 clear: sample the atlas tile * face shade * tint (leaves/grass-top
//    foliage tint, or untinted blocks/flowers).
// The two bit-20 cases are told apart by whether the overlay tile is non-zero
// (the solid path packs no tiles at all).

struct MvpUniform {
    mvp: mat4x4<f32>,
};

// Keep in sync with `block.wgsl` / `render::lighting` (dark cave floor).
const SKY_MIN: f32 = 0.02;
const FINAL_MIN: f32 = 0.006;
const SKY_GAMMA: f32 = 3.0;

@group(0) @binding(0) var<uniform> m: MvpUniform;
// uv-rect table identical to block.wgsl: (u0,v0,u1,v1) per tile. SELECT only.
@group(0) @binding(1) var<uniform> uv_rects: array<vec4<f32>, 256>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    @location(1) tint: vec3<f32>,
    // bits 0..8 = tile id, 8..10 = corner, 10..12 = shade index, 12..20 = overlay
    // tile, 20 = flag (solid-color OR has grass-side overlay), 21..23 = AO,
    // 23..29 = skylight, 29..32 = UV mode (unused by this transformed-item path).
    @location(2) packed: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) light: f32,
    // bit-20 flag (set for solid hand OR grass-side overlay).
    @location(3) @interpolate(flat) flag: u32,
    // overlay (grass-side) uv, sampled when `overlay_tile` is non-zero.
    @location(4) uv2: vec2<f32>,
    // overlay tile id (0 = none); disambiguates the solid vs overlay flag use.
    @location(5) @interpolate(flat) overlay_tile: u32,
};

// Shrink a tile rect (u0,v0,u1,v1) toward its centre by half a texel on every
// edge. One tile is 16 texels wide/tall, so a half-texel is (rect_span)*(0.5/16).
// World blocks (block.wgsl) are large and never sample exactly at a tile edge, so
// they need no inset. But the small, rotated, magnified iso icons + held hand
// (this shader) DO interpolate uv right at the tile boundary, and with a full-tile
// rect those edge fragments bleed into the ADJACENT atlas tile (a 1px sliver of a
// wrong texture down the centre seam where the cube faces meet). Insetting by half
// a texel keeps every reconstructed uv strictly inside its own tile; half a texel
// of a 16px tile is imperceptible, so the texture is not visibly shrunk.
fn inset_tile(r: vec4<f32>) -> vec4<f32> {
    let inset = (vec2<f32>(r.z - r.x, r.w - r.y)) * (0.5 / 16.0);
    return vec4<f32>(r.x + inset.x, r.y + inset.y, r.z - inset.x, r.w - inset.y);
}

// Same corner mapping as block.wgsl: 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
fn corner_uv(r: vec4<f32>, corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(r.x, r.w); }
    if (corner == 1u) { return vec2<f32>(r.z, r.w); }
    if (corner == 2u) { return vec2<f32>(r.z, r.y); }
    return vec2<f32>(r.x, r.y);
}

@vertex
fn vs_model(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = m.mvp * vec4<f32>(in.pos, 1.0);

    let tile = in.packed & 0xFFu;
    let corner = (in.packed >> 8u) & 0x3u;
    let shade_idx = (in.packed >> 10u) & 0x3u;
    let overlay_tile = (in.packed >> 12u) & 0xFFu;
    let ao = (in.packed >> 21u) & 0x3u;
    let sky6 = (in.packed >> 23u) & 0x3Fu;

    // Inset the tile rect by half a texel before reconstructing the corner uv so
    // edge fragments of the magnified iso icons never sample the neighbour tile
    // (see inset_tile). Applied to BOTH the base uv and the grass-side overlay uv2.
    out.uv = corner_uv(inset_tile(uv_rects[tile]), corner);
    out.uv2 = corner_uv(inset_tile(uv_rects[overlay_tile]), corner);
    // Mirror of block.wgsl lighting: directional shade * AO * skylight.
    var shades = array<f32, 4>(1.0, 0.85, 0.75, 0.55);
    var ao_lut = array<f32, 4>(0.25, 0.45, 0.70, 1.0);
    let sky = f32(sky6) / 63.0;
    let sky_term = mix(SKY_MIN, 1.0, pow(sky, SKY_GAMMA));
    out.light = max(FINAL_MIN, shades[shade_idx] * ao_lut[ao] * sky_term);
    out.tint = in.tint;
    out.flag = (in.packed >> 20u) & 0x1u;
    out.overlay_tile = overlay_tile;
    return out;
}

@fragment
fn fs_model(in: VsOut) -> @location(0) vec4<f32> {
    if (in.flag == 1u && in.overlay_tile != 0u) {
        // Grass-block side: untinted dirt base + biome-tinted grayscale grass-side
        // overlay, composited by the overlay's alpha (mirror of block.wgsl
        // fs_opaque) so the side greens to match the tinted top.
        let base = textureSample(atlas, samp, in.uv);
        let ov = textureSample(atlas, samp, in.uv2);
        let rgb = mix(base.rgb, ov.rgb * in.tint, ov.a);
        return vec4<f32>(rgb * in.light, 1.0);
    }
    if (in.flag == 1u) {
        // Skin hand / solid cuboid: output the vertex color, shaded per face so
        // the cuboid reads as a 3D shape. Fully opaque.
        return vec4<f32>(in.tint * in.light, 1.0);
    }
    let tex = textureSample(atlas, samp, in.uv);
    // Flat sprite items (flowers) and leaf cutouts: drop transparent texels so the
    // billboard cuts out cleanly. Block cubes are full-alpha so this is a no-op.
    if (tex.a < 0.5) { discard; }
    return vec4<f32>(tex.rgb * in.tint * in.light, tex.a);
}
