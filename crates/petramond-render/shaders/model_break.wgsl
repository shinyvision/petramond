// model_break: the destroy crack over a bbmodel block.
//
// Draws the SAME `ModelVertex` stream the model pass drew (vs_world_model's
// geometry, vertex for vertex), so the decal's depth is the model's own — there
// is no second, approximate crack shell to misalign. group(0) and group(1) are
// the model pipeline's (frame uniforms; the model atlas). group(2) is this
// pass's own: the frame's crack masks plus the BLOCK atlas, where the destroy
// tiles live.
//
// A fragment is cracked only if it is BOTH claimed by a cracked model's world
// outline box and on a texel the model actually draws — cutout texels discard,
// so no crack floats in a model's empty air. The destroy tile is projected
// across the whole outline box (one tile per model), so a workbench wears one
// continuous crack pattern instead of a complete crack per cube.
//
// CLAIMING IS SIDED, not a plain point-in-box test. Two models placed against
// each other share a boundary plane exactly, and a tight outline box can even
// round a ULP past it, so a point test claims the NEIGHBOUR's faces on that
// plane as readily as the cracked model's own (2026-09-22: the plank of the
// chiseling station south of a cracked one changed colour with every stage).
// The surface itself disambiguates them: a face belongs to the solid its
// outward normal points AWAY from, so the test nudges the fragment a hair back
// along its own normal and asks whether THAT point is in the box. The cracked
// model's own boundary faces move inside it; a neighbour's move away from it.

struct ModelCrack {
    // xyz = the model's world outline min / max, relative to the render origin.
    lo: vec4<f32>,
    hi: vec4<f32>,
    // The stage's destroy tile in the block atlas: (u0, v0, u1, v1).
    rect: vec4<f32>,
};

struct ModelCracks {
    entries: array<ModelCrack, 4>,
    // x = live entries.
    count: vec4<u32>,
};

@group(2) @binding(0) var<uniform> cracks: ModelCracks;
@group(2) @binding(1) var crack_atlas: texture_2d<f32>;
@group(2) @binding(2) var crack_samp: sampler;

struct MbOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) view: vec3<f32>,
    @location(2) world_y: f32,
    @location(3) @interpolate(flat) animation: vec3<f32>,
};

/// How far back along its own normal a fragment is pushed before the box test:
/// far enough to clear the ULP of a tight outline box's own corner, far smaller
/// than any authored part's thickness (a 1-texel plank is 0.0625).
const CRACK_SIDE_NUDGE: f32 = 0.001;

// The clip position is computed EXACTLY as `vs_world_model` computes it (same
// expression on the same vertex), so the decal is depth-coincident by
// construction rather than by a tuned inflation.
@vertex
fn vs_model_break(in: WmIn) -> MbOut {
    var out: MbOut;
    let a = model_animation(in.tint >> 24u);
    let phase = u.fog.z * a.z;
    let frame = u32(floor(phase)) % u32(a.y);
    out.animation = vec3<f32>(f32(frame) * a.x,
        f32((frame + 1u) % u32(a.y)) * a.x, fract(phase) * a.w);
    let local_pos = vec3<f32>(in.col_origin.xyz - u.render_origin.xyz) + in.pos;
    out.clip = u.view_proj * vec4<f32>(local_pos, 1.0);
    out.uv = in.uv;
    out.view = local_pos - u.cam_pos.xyz;
    out.world_y = in.pos.y;
    return out;
}

// The two box-space axes the crack projects along for a face pointing `n`: the
// dominant axis of the normal drops out, so every face takes the slice of the
// tile that its own position covers and the pattern continues across the
// corner between two faces.
fn crack_plane(n: vec3<f32>, v: vec3<f32>) -> vec2<f32> {
    let a = abs(n);
    if (a.x >= a.y && a.x >= a.z) { return vec2<f32>(v.z, v.y); }
    if (a.y >= a.z) { return vec2<f32>(v.x, v.z); }
    return vec2<f32>(v.x, v.y);
}

@fragment
fn fs_model_break(in: MbOut) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow, so the discards below cannot
    // strand the gradients the projection's mip selection needs.
    let p = in.view + u.cam_pos.xyz;
    let dpx = dpdx(p);
    let dpy = dpdy(p);
    let tex_alpha = sample_model_texture(in.uv, in.animation).a;

    // Outward normal: the pass culls back faces, so the visible surface's
    // outward side is the one facing the camera.
    let cn = cross(dpx, dpy);
    var n = cn / max(length(cn), 0.00001);
    n = select(n, -n, dot(n, in.view) > 0.0);

    // The sided claim: a point a hair back along the normal sits in the solid
    // this surface bounds. Only the model that solid belongs to can crack it.
    let inside = p - n * CRACK_SIDE_NUDGE;
    var lo = vec3<f32>(0.0);
    var span = vec3<f32>(1.0);
    var rect = vec4<f32>(0.0);
    var hit = false;
    for (var i = 0u; i < cracks.count.x; i = i + 1u) {
        let c = cracks.entries[i];
        if (all(inside >= c.lo.xyz) && all(inside <= c.hi.xyz)) {
            lo = c.lo.xyz;
            span = max(c.hi.xyz - c.lo.xyz, vec3<f32>(0.0001));
            rect = c.rect;
            hit = true;
            break;
        }
    }
    if (!hit) { discard; }
    // Geometry only: a texel the model does not draw takes no crack.
    if (tex_alpha < 0.5) { discard; }

    let size = rect.zw - rect.xy;
    // One tile across the whole outline box. v grows DOWN the atlas, so the
    // box-space vertical axis is flipped into it.
    let st = crack_plane(n, (p - lo) / span);
    let uv = rect.xy + vec2<f32>(st.x, 1.0 - st.y) * size;
    let gx = crack_plane(n, dpx / span) * vec2<f32>(size.x, -size.y);
    let gy = crack_plane(n, dpy / span) * vec2<f32>(size.x, -size.y);
    let tex = textureSampleGrad(crack_atlas, crack_samp, uv, gx, gy);

    // MULTIPLY blend, as break_overlay.wgsl: transparent texels are WHITE (x1 =
    // no change), crack texels darken the model's own shading.
    var crack = mix(vec3<f32>(1.0), tex.rgb, tex.a);
    // Fade the darkening toward the multiply identity on the same haze curve the
    // surface under it already took, so the crack melts into fog with the model.
    let dist = length(in.view);
    var f: f32;
    if (u.fog.w > 0.5) {
        f = clamp((dist - u.fog.x) / (u.fog.y - u.fog.x), 0.0, 1.0);
    } else {
        f = atmosphere_amount(
            dist,
            u.fog.x,
            u.fog.y,
            in.world_y,
            u.cam_pos.y + f32(u.render_origin.y),
        );
    }
    crack = mix(crack, vec3<f32>(1.0), f);
    return vec4<f32>(crack, 1.0);
}
