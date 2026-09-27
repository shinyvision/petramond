// World marks: keep the world's EYE DEPTH for the marks pass, at the window's
// frame size, before the hand pass clears the depth buffer. One texel per
// frame pixel holds the farthest depth under it (every MSAA sample, every
// supersample), as a distance along the view axis — what a mark fragment's
// clip w compares against.
//
// `full_depth` and `scene_depth_max` are prepended for the scene's sample
// count.

#import petramond::frame

struct MarkParams {
    viewport: vec4<f32>,
    depth_size: vec4<f32>,
};

@group(0) @binding(1) var<uniform> u: Uniforms;
@group(0) @binding(2) var<uniform> params: MarkParams;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vi) / 2) * 4.0 - 1.0;
    let y = f32(i32(vi) % 2) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_depth(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let out_size = vec2<i32>(params.depth_size.xy);
    let scene = vec2<i32>(textureDimensions(full_depth));
    let px = vec2<i32>(pos.xy);
    let lo = px * scene / out_size;
    let hi = max(lo, ((px + vec2<i32>(1)) * scene - vec2<i32>(1)) / out_size);
    var d = 0.0;
    // Supersampling tops out at 4x4 scene texels per frame pixel.
    for (var y = lo.y; y <= min(hi.y, lo.y + 3); y++) {
        for (var x = lo.x; x <= min(hi.x, lo.x + 3); x++) {
            d = max(d, scene_depth_max(min(vec2<i32>(x, y), scene - vec2<i32>(1))));
        }
    }
    let ndc = vec2<f32>(
        pos.x / params.depth_size.x * 2.0 - 1.0,
        1.0 - pos.y / params.depth_size.y * 2.0,
    );
    // The eye depth is the clip w of the world point this depth stands for.
    let h = u.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return vec4<f32>(1.0 / h.w, 0.0, 0.0, 0.0);
}
