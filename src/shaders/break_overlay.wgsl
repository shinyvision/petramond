// break_overlay: the cracked-block destroy overlay.
//
// Draws the targeted block's cube faces (built CPU-side at the block's exact
// integer cell, coincident with the chunk mesh — no inflation) textured with a
// Tile::DestroyStage{0..9} grayscale crack and MULTIPLY-blends it over the world
// (result = src.rgb * dst.rgb) so the cracks darken the block face instead of
// alpha-compositing a flat overlay. group(0): Uniforms (view_proj at binding 0) +
// the shared uv_rects table (binding 1) — the SAME bind group as the block
// pipeline. group(1) is the block atlas (the destroy tiles live in it).
//
// Vertex format is the shared 32-byte mesh::Vertex (packed2's light bits are
// unused here — the crack needs no light; its cell-local uv bits ARE read for
// stair quads). We only need uv reconstruction (SELECT from uv_rects — never
// recompute), so this is a trimmed copy of the block vertex stage. The crack cube is coincident with the block faces; the pipeline
// draws it depth LessEqual / no-write with a small polygon offset toward the camera
// (BREAK_DEPTH_BIAS in pipeline.rs) so the decal wins the depth tie cleanly — see
// that constant for why the offset is needed (the mesher's per-AO diagonal flip).

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos:   vec4<f32>,
    fog:       vec4<f32>,
    fog_color: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<uniform> uv_rects: array<vec4<f32>, 256>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

// Mirror of block.wgsl's CELL_LOCAL UV mode (packed bits 29..32): the vertex
// carries an explicit tile-local UV in packed2 bits 6..11 / 11..16 (1/16ths).
// Stair crack quads use it so the crack decal is continuous across the stair
// instead of restarting the tile per quad.
const UV_MODE_CELL_LOCAL: u32 = 3u;

struct VsIn {
    @location(0) pos:  vec3<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) packed: u32,
    // Second packed word: bits 6..16 = cell-local uv (CELL_LOCAL mode only);
    // the light bits are unused here (the crack needs no light).
    @location(3) packed2: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dist: f32,
    // Absolute world height, for the atmosphere's altitude thinning.
    @location(2) world_y: f32,
};

// Same corner mapping as block.wgsl: 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
fn corner_uv(r: vec4<f32>, corner: u32) -> vec2<f32> {
    if (corner == 0u) { return vec2<f32>(r.x, r.w); }
    if (corner == 1u) { return vec2<f32>(r.z, r.w); }
    if (corner == 2u) { return vec2<f32>(r.z, r.y); }
    return vec2<f32>(r.x, r.y);
}

@vertex
fn vs_break(in: VsIn) -> VsOut {
    var out: VsOut;
    let local_pos = in.pos - u.render_origin.xyz;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    let tile = in.packed & 0xFFu;
    let corner = (in.packed >> 8u) & 0x3u;
    let uv_mode = (in.packed >> 29u) & 0x7u;
    let r = uv_rects[tile];
    if (uv_mode == UV_MODE_CELL_LOCAL) {
        let c = vec2<f32>(
            f32((in.packed2 >> 6u) & 0x1Fu),
            f32((in.packed2 >> 11u) & 0x1Fu),
        ) / 16.0;
        out.uv = mix(r.xy, r.zw, c);
    } else {
        out.uv = corner_uv(r, corner);
    }
    // World-space camera distance, for the fog fade in the fragment stage (matches
    // block.wgsl so the crack fades on the same curve as the surface it sits on).
    out.dist = length(u.cam_pos.xyz - local_pos);
    out.world_y = in.pos.y;
    return out;
}

@fragment
fn fs_break(in: VsOut) -> @location(0) vec4<f32> {
    // The destroy tiles are grayscale + alpha: the crack pixels have alpha, the
    // rest is transparent. Under MULTIPLY blend (src.rgb * dst.rgb) we want
    // transparent texels to be WHITE (×1 = no change) and crack texels to be dark,
    // so mix toward the crack colour by its own alpha. Output alpha is unused (the
    // blend preserves the destination alpha).
    let tex = textureSample(atlas, samp, in.uv);
    var crack = mix(vec3<f32>(1.0), tex.rgb, tex.a);
    // Fog fade: the block underneath has already hazed toward the atmosphere
    // (block.wgsl), or toward the tight linear murk underwater. Fade the crack's
    // darkening toward 1.0 (multiply identity) by the same total amount so it
    // melts into the haze with the surface instead of staying a hard dark
    // pattern floating in fog.
    var f: f32;
    if (u.fog.w > 0.5) {
        f = clamp((in.dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    } else {
        f = atmosphere_amount(
            in.dist,
            u.fog.x,
            u.fog.y,
            in.world_y,
            u.cam_pos.y + u.render_origin.y,
        );
    }
    crack = mix(crack, vec3<f32>(1.0), f);
    return vec4<f32>(crack, 1.0);
}
