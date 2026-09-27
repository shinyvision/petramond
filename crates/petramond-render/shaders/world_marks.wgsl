// World marks: lines and pinned art at world points, drawn on the WINDOW over
// the finished frame (after its capture point), sized in window pixels.
//
// Anchors arrive render-relative; the vertex stage projects them with the
// frame's own view_proj and grows each mark in pixels. A line clips to the
// near plane in clip space before it is widened, so a segment passing behind
// the eye still draws the part in front of it.
//
// Occlusion: the frame's depth is gone by the time the window draws (the hand
// pass clears it), so `fs_depth` keeps the world's EYE DEPTH at the window's
// frame size before the hand draws, and a mark fragment farther than that is
// drawn at its `occluded` opacity.

#import petramond::frame

// `viewport` = the frame's rect on the target (x, y, w, h px); `depth_size`
// = the kept depth's size (w, h).
struct MarkParams {
    viewport: vec4<f32>,
    depth_size: vec4<f32>,
};

@group(0) @binding(0) var art: texture_2d<f32>;
@group(0) @binding(1) var art_samp: sampler;
@group(1) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(1) var<uniform> params: MarkParams;
@group(1) @binding(2) var world_eye: texture_2d<f32>;

// A fragment this much (absolute blocks + a fraction of its distance) behind
// the world still counts as in front: a line laid on a face must not flicker.
const OCCLUSION_SLACK: f32 = 0.02;
const OCCLUSION_SLACK_PER_BLOCK: f32 = 0.002;

struct VsIn {
    @location(0) at: vec3<f32>,
    @location(1) other: vec3<f32>,
    @location(2) offset: vec2<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) style: vec2<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) eye: f32,
    @location(3) @interpolate(flat) occluded: f32,
};

fn culled() -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(0.0, 0.0, -1.0, 1.0);
    out.uv = vec2<f32>(-1.0, -1.0);
    out.color = vec4<f32>(0.0);
    out.eye = 0.0;
    out.occluded = 0.0;
    return out;
}

// Move `p` along the segment to the near plane (clip z = 0) when it is behind it.
fn to_near(p: vec4<f32>, q: vec4<f32>) -> vec4<f32> {
    if (p.z >= 0.0) {
        return p;
    }
    return mix(p, q, p.z / (p.z - q.z));
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let half_px = params.viewport.zw * 0.5;
    var here = u.view_proj * vec4<f32>(in.at, 1.0);
    var grow: vec2<f32>;
    if (in.style.y > 0.5) {
        var there = u.view_proj * vec4<f32>(in.other, 1.0);
        if (here.z < 0.0 && there.z < 0.0) {
            return culled();
        }
        let a = to_near(here, there);
        let b = to_near(there, here);
        here = a;
        // Pixel space, y up.
        let d = a.xy / a.w * half_px - b.xy / b.w * half_px;
        let len = length(d);
        let dir = select(vec2<f32>(1.0, 0.0), d / len, len > 1e-4);
        let normal = vec2<f32>(-dir.y, dir.x);
        let half_width = in.offset.y;
        // Square caps: each end reaches half a width past its point, so the
        // segments of a path meet without a notch.
        grow = normal * in.offset.x * half_width + dir * half_width;
    } else {
        if (here.z < 0.0) {
            return culled();
        }
        grow = vec2<f32>(in.offset.x, -in.offset.y);
    }
    var out: VsOut;
    out.pos = vec4<f32>(here.xy + grow / half_px * here.w, here.zw);
    out.uv = in.uv;
    out.color = in.color;
    out.eye = here.w;
    out.occluded = in.style.x;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = in.color;
    if (in.uv.x >= 0.0) {
        color = color * textureSampleLevel(art, art_samp, in.uv, 0.0);
    }
    if (in.occluded < 1.0) {
        let rel = (in.pos.xy - params.viewport.xy) / params.viewport.zw;
        let size = vec2<i32>(params.depth_size.xy);
        let texel = clamp(vec2<i32>(rel * params.depth_size.xy), vec2<i32>(0), size - 1);
        let world = textureLoad(world_eye, texel, 0).r;
        if (in.eye > world + OCCLUSION_SLACK + world * OCCLUSION_SLACK_PER_BLOCK) {
            color.a = color.a * in.occluded;
        }
    }
    if (color.a <= 0.0) {
        discard;
    }
    return color;
}
