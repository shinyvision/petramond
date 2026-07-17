// Downsample the full-res scene depth into the half-res environment
// target's depth: the MAX (farthest) of each 2x2 block, so a volumetric
// marching against the half-res depth never loses sky coverage at
// silhouette edges — the depth-aware composite (env_composite.wgsl)
// resolves those edges per full-res pixel.

@group(0) @binding(0) var full_depth: texture_depth_2d;

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
fn fs_main(in: VsOut) -> @builtin(frag_depth) f32 {
    let dims = vec2<i32>(textureDimensions(full_depth));
    let base = vec2<i32>(in.pos.xy) * 2;
    var d = 0.0;
    for (var oy = 0; oy < 2; oy++) {
        for (var ox = 0; ox < 2; ox++) {
            let p = min(base + vec2<i32>(ox, oy), dims - 1);
            d = max(d, textureLoad(full_depth, p, 0));
        }
    }
    return d;
}
