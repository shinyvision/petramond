// Upsample the half-res environment (volumetric) accumulation onto the
// full-res scene, depth-aware: where the 2x2 half-res neighbourhood spans a
// depth edge (a mountain silhouette against sky), the full-res pixel takes
// the single half-res sample whose depth best matches its own instead of
// bilinearly bleeding cloud across the edge; depth-flat regions use plain
// bilinear (soft volumetrics upsample invisibly there). Output stays
// premultiplied and blends over the scene exactly as the full-res
// environment passes used to.

@group(0) @binding(0) var env_tex: texture_2d<f32>;
@group(0) @binding(1) var env_samp: sampler;
@group(0) @binding(2) var half_depth: texture_depth_2d;
@group(0) @binding(3) var full_depth: texture_depth_2d;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(vi) / 2) * 4.0 - 1.0;
    let y = f32(i32(vi) % 2) * 4.0 - 1.0;
    out.pos = vec4<f32>(x, y, 1.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let fd = textureLoad(full_depth, vec2<i32>(in.pos.xy), 0);
    let hdims = vec2<i32>(textureDimensions(half_depth));
    // This pixel's position in half-res texel space, and the 2x2 texel
    // neighbourhood bracketing it.
    let hpos = in.pos.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(hpos));
    var best = vec4<f32>(0.0);
    var best_err = 1e9;
    var dmin = 1.0;
    var dmax = 0.0;
    for (var oy = 0; oy < 2; oy++) {
        for (var ox = 0; ox < 2; ox++) {
            let hp = clamp(base + vec2<i32>(ox, oy), vec2<i32>(0), hdims - 1);
            let hd = textureLoad(half_depth, hp, 0);
            dmin = min(dmin, hd);
            dmax = max(dmax, hd);
            let err = abs(hd - fd);
            if (err < best_err) {
                best_err = err;
                best = textureLoad(env_tex, hp, 0);
            }
        }
    }
    if (dmax - dmin < 1e-3) {
        let uv = (hpos + 0.5) / vec2<f32>(hdims);
        return textureSampleLevel(env_tex, env_samp, uv, 0.0);
    }
    return best;
}
