// grade: full-screen colour grade over the finished world image.
//
// The world (sky → terrain → entities → hand) renders into an offscreen scene
// texture; this pass samples it (bilinear) and writes the graded result to the
// swapchain. Screen chrome (crosshair, UI) draws AFTER the grade so interface
// colours stay exact. Sampling — not textureLoad — because the scene texture
// may be SMALLER than the swapchain (the render_scale client setting): the
// grade pass doubles as the upscale. At scale 1.0 every sample lands exactly
// on a texel centre, identical to a load.
//
// The grade shapes the vibrant storybook tonal range:
//  - a gentle S-curve for soft contrast without crushing the flat colour fields,
//  - VIBRANCE instead of flat saturation: muted pixels get pushed toward colour
//    hard, already-saturated pixels (grass, flowers, sky) are protected, so the
//    image pops without ever clipping into neon,
//  - split-toning: highlights lean warm cream, shadows lean cool blue — the
//    painterly warm-light/cool-shadow statement, applied globally,
//  - NO black lift: the light pipeline owns the black floor (block.wgsl
//    FINAL_MIN / SKY_MIN, plus the bright daytime ambient). An additive grade
//    floor converged caves and night below ~0.03 linear to one flat gray.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_samp: sampler;

// Rec.709 luma weights.
const GRADE_LUMA_W: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// How much of the smoothstep S-curve blends over the linear image.
const GRADE_CONTRAST: f32 = 0.20;
// Vibrance: saturation push for fully-muted pixels; fades to 0 as a pixel's own
// chroma approaches full (SweetFX-style smart saturation).
const GRADE_VIBRANCE: f32 = 0.30;
// Split-tone tints (multiplicative, luma-weighted). Kept near-white: this is a
// lean, not a wash.
const GRADE_SHADOW_TINT: vec3<f32> = vec3<f32>(0.94, 0.97, 1.06);
const GRADE_HIGHLIGHT_TINT: vec3<f32> = vec3<f32>(1.05, 1.01, 0.95);
// Black floor after everything else. ZERO on purpose: an absolute additive
// floor is why caves and night once read washed out — day shadows never reach
// it (their ambient is high), so it only ever lifted the darks that were meant
// to be dark. Raise only for a deliberate faded-film look.
const GRADE_LIFT: f32 = 0.0;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_grade(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>( 3.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    var out: VsOut;
    let p = positions[vertex_index];
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_grade(in: VsOut) -> @location(0) vec4<f32> {
    let src = textureSample(scene, scene_samp, in.uv).rgb;
    // Soft S-curve: smoothstep of the clamped colour, blended lightly over the
    // original so contrast firms up without banding or crushed shadows.
    let cl = clamp(src, vec3<f32>(0.0), vec3<f32>(1.0));
    let curve = cl * cl * (3.0 - 2.0 * cl);
    var c = mix(src, curve, GRADE_CONTRAST);
    // Vibrance around luma: the push scales with how UNsaturated the pixel is.
    var luma = dot(c, GRADE_LUMA_W);
    let sat = max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
    c = mix(vec3<f32>(luma), c, 1.0 + GRADE_VIBRANCE * clamp(1.0 - sat, 0.0, 1.0));
    // Split-toning: cool the shadows, warm the highlights.
    luma = dot(c, GRADE_LUMA_W);
    let tone = smoothstep(0.12, 0.78, luma);
    c *= mix(GRADE_SHADOW_TINT, GRADE_HIGHLIGHT_TINT, tone);
    // Lifted blacks, applied last so the floor is absolute.
    c = c * (1.0 - GRADE_LIFT) + vec3<f32>(GRADE_LIFT);
    return vec4<f32>(c, 1.0);
}
