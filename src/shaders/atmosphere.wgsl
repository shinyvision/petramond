// Shared aerial-perspective atmosphere for world-space shaders.
//
// This file is NOT a standalone module: pipeline.rs prepends it (concat!) to the
// source of every shader that fogs world geometry (block, mob, particles,
// break_overlay, builtin sky). WGSL has no include mechanism, so the shared look
// lives here once instead of drifting per shader. daynight_sky.wgsl (a runtime
// asset, so it cannot be concat!-ed) mirrors the haze-colour formula and the
// sun-glow constants — keep them in sync.
//
// The look: a CLEAR, saturated midrange that
// dissolves late into a luminous haze — the haze is the biome fog colour with
// its chroma pushed UP, so distance glows like painted air instead of washing
// out to milk (Firewatch/Sable-style authored fog, not scattering). Underwater
// keeps the tight linear murk — callers branch on the underwater flag before
// calling in here.
//
// CONTRACT: atmosphere_amount() returns exactly 1.0 for dist >= fog_end. Terrain
// visibility is fog-distance culled (TERRAIN_FOG_CULL_PAD), so a pixel that is
// not fully hazed at fog_end would pop at the cull boundary. The terminal
// smoothstep guarantees this; keep every other term composited UNDER it.

// Rec.709 luma weights.
const ATMOS_LUMA_W: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// Peak strength of the near/mid aerial haze (the terminal fog band still runs
// to 1.0 on top of it). The quartic onset keeps the midrange crisp; the far
// cap is generous so distant shadow masses LIGHTEN into the air instead of
// looming as dark walls.
const ATMOS_AERIAL_MAX: f32 = 0.42;
// Quartic-depth exponential of the aerial haze across [0, fog_end]: flat for
// the first ~half of the range, rising late (Complementary-style border curve).
const ATMOS_AERIAL_CURVE: f32 = 3.5;
// Haze is densest near sea level and thins with altitude: blocks above sea
// level over which it falls by e×. Uses the fragment/camera midpoint height so
// valleys sit in haze while nearby peaks (and views from peaks) rise out of
// it. The protection fades with distance — a far mountain must dissolve into
// luminous air no matter how tall it is, or it looms as a dark wall.
const ATMOS_SEA_Y: f32 = 63.0;
const ATMOS_HAZE_SCALE_HEIGHT: f32 = 240.0;
// Chroma push of the haze colour: distance saturates INTO the fog hue rather
// than desaturating out of it, keeping far silhouettes glowing against the
// sky. Per-channel and red-anchored: the blue channel is pushed hardest away
// from luma so sky-blue fogs deepen toward azure (not cyan — red stays put)
// and warm desert fogs deepen toward gold. Mirrored by daynight_sky.wgsl.
const ATMOS_CHROMA_BOOST: vec3<f32> = vec3<f32>(0.0, 0.12, 0.55);
// Small brightness lift so fully-hazed terrain sits a touch ABOVE the sky
// horizon it meets: luminous air, never a grey curtain. Mirrored likewise.
const ATMOS_HAZE_LIFT: f32 = 1.04;
// Fog-dome shading: haze seen along flat sightlines glows at full strength;
// steep sightlines (peaks from below, valleys from a summit) cool and deepen
// toward this tint, so the haze reads as a dome of air, not a screen overlay.
const ATMOS_DOME_TINT: vec3<f32> = vec3<f32>(0.87, 0.92, 1.08);
// Sun-glow: width (cosine power), warm peach tint, and cap of the haze looking
// sunward. daynight_sky.wgsl mirrors these for its horizon — keep in sync.
const ATMOS_SUN_GLOW_POW: f32 = 8.0;
const ATMOS_SUN_GLOW_WARM: vec3<f32> = vec3<f32>(1.22, 1.04, 0.84);
const ATMOS_SUN_GLOW_MAX: f32 = 0.70;

// --- Sun-directional face lighting ---
// Illustrative colour-ramp shading for terrain faces carrying a normal code
// (block.wgsl, packed2 bits 16..19): a lit face bathes in warm sun colour and a
// shadow face falls to a COOL LUMINOUS BLUE — a colour choice, not a darkening
// (the TF2/Journey rule: shadows shift hue, so they stay airy and readable).
// Down-faces get a warmer, earthier ambient (ground bounce) so overhangs and
// canopies separate from blue side-shadow. The lit ramp is cel-banded
// (cel.wgsl): faces quantize into shadow / half-lit / lit stages, splitting the
// six cube faces into a confident poster-flat toon statement. At night
// everything relaxes to a flat diffuse level so moonlit terrain stays readable.
const SUN_LIT_COLOR: vec3<f32> = vec3<f32>(1.08, 1.03, 0.92);
const SUN_SHADOW_COLOR: vec3<f32> = vec3<f32>(0.76, 0.82, 0.97);
const SUN_GROUND_COLOR: vec3<f32> = vec3<f32>(0.66, 0.63, 0.58);
const SUN_NIGHT_FLAT: f32 = 0.90;

// Unit normal for a face code 1..=6 (Face::normal_code order: +X −X +Y −Y +Z
// −Z). Code 0 has no direction; callers keep their legacy shading for it.
fn face_normal(code: u32) -> vec3<f32> {
    var normals = array<vec3<f32>, 7>(
        vec3<f32>(0.0, 0.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, -1.0),
    );
    return normals[min(code, 6u)];
}

// Callers must fade this ramp by SKY exposure (block.wgsl mixes it over the
// neutral shade table by the vertex sky light): the warm/cool split is
// daylight, and applying it underground paints sunless caves warm/cool.
fn sun_face_shade(n: vec3<f32>, sun_dir: vec3<f32>, daylight: f32) -> vec3<f32> {
    let lambert = max(dot(n, sun_dir), 0.0);
    // Cel-banded ramp (cel.wgsl CEL_SUN): shadow -> half-lit -> lit in discrete
    // stages, so faces step through a mid tone as the sun sweeps instead of
    // fading smoothly.
    let lit = cel_band(CEL_SUN, lambert);
    let ground = clamp(-n.y, 0.0, 1.0);
    let ambient = mix(SUN_SHADOW_COLOR, SUN_GROUND_COLOR, ground);
    let day = mix(ambient, SUN_LIT_COLOR, lit);
    return mix(vec3<f32>(SUN_NIGHT_FLAT), day, daylight);
}

// Total haze fraction at `dist`. `world_y`/`cam_y` are ABSOLUTE world heights.
fn atmosphere_amount(dist: f32, fog_start: f32, fog_end: f32, world_y: f32, cam_y: f32) -> f32 {
    // Terminal band: the hard guarantee of full haze at the culling distance.
    let terminal = smoothstep(fog_start, fog_end, dist);
    // Aerial term: near-zero for the first half of the range, rising late;
    // denser low, thinner up high.
    let d = clamp(dist / fog_end, 0.0, 1.0);
    let d2 = d * d;
    let depth = 1.0 - exp(-d2 * d2 * ATMOS_AERIAL_CURVE);
    let alt = max(0.5 * (world_y + cam_y) - ATMOS_SEA_Y, 0.0);
    let alt_thin = mix(exp(-alt / ATMOS_HAZE_SCALE_HEIGHT), 1.0, d2);
    let aerial = ATMOS_AERIAL_MAX * depth * alt_thin;
    // Aerial haze composites UNDER the terminal fog, not summed inside the band.
    return clamp(terminal + aerial * (1.0 - terminal), 0.0, 1.0);
}

// Haze colour for a view direction: the biome fog colour pushed toward its own
// hue (chroma boost) and lifted to luminance, shaped by the fog dome, then
// warmed toward the sun. sky.wgsl calls this for its horizon, so the terrain
// haze meets the builtin sky invisibly; daynight_sky.wgsl mirrors the formula.
fn atmosphere_haze_color(
    view_dir: vec3<f32>,
    base: vec3<f32>,
    sun_dir: vec3<f32>,
    daylight: f32,
) -> vec3<f32> {
    let luma = dot(base, ATMOS_LUMA_W);
    var haze = (base + (base - vec3<f32>(luma)) * ATMOS_CHROMA_BOOST) * ATMOS_HAZE_LIFT;
    // Hue-preserving gamut clip: scale the whole colour back when a channel
    // overflows. Hard-clamping instead would skew the g/b ratio toward cyan.
    haze /= max(1.0, max(haze.r, max(haze.g, haze.b)));
    let horizon = 1.0 - clamp(abs(view_dir.y) * 1.8, 0.0, 1.0);
    haze *= mix(ATMOS_DOME_TINT, vec3<f32>(1.0), horizon);
    let toward = max(dot(view_dir, sun_dir), 0.0);
    let glow = pow(toward, ATMOS_SUN_GLOW_POW) * daylight * ATMOS_SUN_GLOW_MAX;
    return mix(haze, haze * ATMOS_SUN_GLOW_WARM, glow);
}

// Blend `color` into the atmosphere. `view` = fragment − camera in render-local
// space (unnormalized); `world_y`/`cam_y` are ABSOLUTE world heights; `sun_dir`
// is the unit sun direction with `daylight` in [0,1].
fn atmosphere_apply(
    color: vec3<f32>,
    view: vec3<f32>,
    world_y: f32,
    cam_y: f32,
    fog_start: f32,
    fog_end: f32,
    base_haze: vec3<f32>,
    sun_dir: vec3<f32>,
    daylight: f32,
) -> vec3<f32> {
    let dist = length(view);
    let f = atmosphere_amount(dist, fog_start, fog_end, world_y, cam_y);
    let haze = atmosphere_haze_color(view / max(dist, 1e-4), base_haze, sun_dir, daylight);
    return mix(color, haze, f);
}
