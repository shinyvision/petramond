// ui: 2D HUD / inventory pass (hotbar, panel, digits, drag cursor).
//
// Vertices are already in NDC (clip x/y in [-1, 1], y up) so the vertex stage is
// a passthrough — the CPU (`render::ui`) does all layout math against the chosen
// integer GUI scale so render and `slot_at_cursor` hit-testing share one source.
//
// group(0) is the GUI sprite atlas (texture + sampler), a SEPARATE atlas from the
// block atlas. ALPHA blend, NO depth (the UI is the last pass).
//
// SOLID-COLOR SENTINEL: a negative `uv.x` flags a solid-color quad (dim
// background, font digits, color fills) — the fragment stage then outputs the
// vertex `color` directly and never samples the atlas. Otherwise it samples the
// gui atlas and multiplies by `color` (so sprites can be tinted / faded).

@group(0) @binding(0) var gui: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,   // NDC position (y up)
    @location(1) uv: vec2<f32>,    // gui-atlas uv, or uv.x < 0 = solid color
    @location(2) color: vec4<f32>, // tint (textured) / fill (solid)
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_ui(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_ui(in: VsOut) -> @location(0) vec4<f32> {
    // Solid-color sentinel: negative u means "no texture, output color".
    if (in.uv.x < 0.0) {
        return in.color;
    }
    let tex = textureSample(gui, samp, in.uv);
    return tex * in.color;
}
