// Cel/toon light banding shared by world-space shaders.
//
// WGSL port of mightymochi's "2D Cel-Toon Shader v2 Plus" for Godot 3.x
// (shaders/cel_shader_plus_all.shader): light is quantized into two or three
// discrete stages, each with a smoothstep edge, instead of a smooth ramp.
// pipeline.rs prepends this file (concat!, like atmosphere.wgsl) to every
// world-shader chain; atmosphere.wgsl's sun_face_shade bands its lambert here,
// block.wgsl bands the interpolated vertex light per fragment.
//
// What ported and what did not:
// - The banding core (first/second stage, per-stage smoothing, min/mid/max
//   light, obj_light_add) ports directly — it is pure intensity math.
// - Rim light ports as a VIEW-ANGLE edge term. The original detects sprite
//   silhouettes by sampling neighbour texel alpha; a chunk mesh has no alpha
//   silhouette, but a face seen at a glancing angle IS the 3D silhouette, so
//   the rim rides N·V instead.
// - fake_light_depth / fake_spot_light / light_fade need a positional 2D light
//   (Godot's LIGHT_VEC / LIGHT_HEIGHT); our sun is directional and block light
//   is baked into vertices, so they have no analog here and are omitted.

struct CelStages {
    first_stage: f32,   // strength where the dark band ends
    first_smooth: f32,  // width of the dark->mid edge
    second_stage: f32,  // strength where the mid band ends; 0.0 = two-band mode
    second_smooth: f32, // width of the mid->lit edge
    min_light: f32,     // dark band: scales the raw strength (and lifts the mid band)
    mid_light: f32,     // mid-band cap; 0.0 = auto (max_light * 0.5)
    max_light: f32,     // lit-band cap
    smooth_mix: f32,    // obj_light_add: mixes raw strength back over the bands
}

// Stage sets (playtest-iterable, like the atmosphere/grade knobs).
// CEL_SUN bands the sun lambert in sun_face_shade: shadow -> half-lit -> lit.
const CEL_SUN: CelStages = CelStages(0.04, 0.12, 0.40, 0.12, 0.0, 0.0, 1.0, 0.0);
// CEL_LIGHT bands the terrain light-LEVEL gradient (sky/torch, WITHOUT AO —
// corners keep their smooth contact shadow) per fragment. Stages sit near the
// raw values they replace so the bands read as soft tone steps, not a remap.
const CEL_LIGHT: CelStages = CelStages(0.45, 0.10, 0.75, 0.10, 0.40, 0.62, 1.0, 0.0);
// Middle-ground softeners for the fragment banding: below the FADE window the
// band ratio blends back to identity, so walking into a cave darkens along the
// original smooth gradient instead of stepping through tone rings; above it,
// only STRENGTH of the banded/raw ratio applies, leaving gentle toon steps
// (canopy light, torch pools) instead of flat posterized bands. STRENGTH 1.0 +
// FADE 0.0/0.0 recovers the fully banded look.
const CEL_LIGHT_FADE_LO: f32 = 0.30;
const CEL_LIGHT_FADE_HI: f32 = 0.55;
const CEL_LIGHT_STRENGTH: f32 = 0.6;

// Rim: additive edge light on faces at glancing view angles (block.wgsl).
const CEL_RIM_ENABLED: bool = true;
const CEL_RIM_FALLOFF: f32 = 3.0;  // higher = thinner rim (rim_thickness analog)
const CEL_RIM_INTENSE: f32 = 0.18; // rim_intense

// The banded intensity for a smooth strength in [0,1]. Faithful to the
// reference's light(): three exclusive bands when second_stage != 0.0, two
// bands otherwise; below first_stage the raw strength is scaled to min_light
// rather than flattened, so the darkest region keeps its gradient.
fn cel_band(s: CelStages, strength: f32) -> f32 {
    var mid = s.mid_light;
    if (s.mid_light == 0.0) { mid = s.max_light * 0.5; }
    var banded: f32;
    if (strength > s.first_stage && s.second_stage == 0.0) {
        banded = min(
            smoothstep(s.first_stage, s.first_stage + max(s.first_smooth, 1e-4), strength)
                + s.min_light,
            s.max_light,
        );
    } else if (strength > s.first_stage && strength < s.second_stage) {
        banded = min(
            smoothstep(s.first_stage, s.first_stage + max(s.first_smooth, 1e-4), strength)
                + s.min_light,
            mid,
        );
    } else if (strength >= s.second_stage && s.second_stage != 0.0) {
        banded = clamp(
            smoothstep(s.second_stage, s.second_stage + max(s.second_smooth, 1e-4), strength)
                + mid,
            mid,
            s.max_light,
        );
    } else {
        banded = strength * s.min_light;
    }
    return banded + strength * s.smooth_mix;
}

// 3D rim: brightens a face as its normal turns away from the viewer, scaled by
// the current light so unlit caves do not rim-glow. `view_dir` points from the
// camera toward the fragment.
fn cel_rim(n: vec3<f32>, view_dir: vec3<f32>, light: vec3<f32>) -> vec3<f32> {
    let facing = max(dot(n, -view_dir), 0.0);
    // CEL_RIM_FALLOFF == 3: e³ as multiplies beats the exp2/log2 pow().
    let e = 1.0 - facing;
    let edge = e * e * e;
    let span = CEL_LIGHT.max_light - CEL_LIGHT.min_light;
    return light * (edge * CEL_RIM_INTENSE * span);
}
