// Volumetric cloud deck — the weather pack's `environment` pass.
//
// A thin horizontal slab (CLOUD_BASE..+CLOUD_THICK) raymarched between its
// analytic plane hits, clamped against the scene depth, so mountains occlude
// clouds and cloud banks drift in front of far ridges. Coverage is the SAME
// closed-form field the weather mod simulates (`weather-core` is this file's
// twin: fmix32 hash, tiling lattice noise, storm remap, TWO sheets sliding
// at different speeds whose saturating sum is the coverage — change one,
// change both), fed by two replicated params:
//   params[0] = weather:wind  [off_x, off_z, wind_x, wind_z]
//   params[1] = weather:sky   [storm, rain_start, feature_size, seed]
//   params[2] = weather:flux  [epoch, epoch_frac, 0, 0]
//
// Lighting: Beer-Lambert extinction with a second wide lobe (multi-scatter
// hack), dual-lobe Henyey-Greenstein phase (soft silver lining), an
// in-scatter "powder" term for flat dark bases, a SUN-INDEPENDENT top rim
// (skylight from the open sky above — crowns stay lit in any view
// direction, even on storm cells) plus a daytime sun CROWN on the very top
// of the deck (view-independent, menace-ramped: it restores the sunlight
// the storm darkening steals from crests), energy-conserving per-step
// integration,
// and a MENACE-keyed extinction boost that opens a little before the rain
// band and saturates with the downpour (its ramp end is weather-core's
// RAIN_RAMP twin) — accumulated cloud is darker cloud, and the color turns
// before the rain arrives. Distant decks dissolve into the luminous horizon
// haze (storybook: distance lightens).

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog: vec4<f32>,        // (start, end, time, underwater)
    fog_color: vec4<f32>,  // rgb = haze color (night-dimmed); w = sky scale
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    water_anim: vec4<u32>,
    sky_color: vec4<f32>,  // rgb = sky-light tint
    sun_dir: vec4<f32>,    // xyz = unit sun direction; w = daylight [0,1]
};

@group(0) @binding(0) var<uniform> u: Uniforms;
struct ShaderParams { values: array<vec4<f32>, 16> };
@group(0) @binding(1) var<uniform> params: ShaderParams;
@group(0) @binding(2) var depth_tex: texture_depth_2d;

// --- tuning (storybook: few, large, PUFFY shapes) ---------------------------
const CLOUD_BASE: f32 = 192.0;
const CLOUD_THICK: f32 = 96.0;
const CLOUD_FAR: f32 = 2400.0;   // march cap; beyond, the deck fades to haze
const STEPS: i32 = 20;
const WRAP: f32 = 65536.0;
const SIGMA_T: f32 = 0.11;       // extinction at density 1 — thick cores go
                                 // optically deep fast (that contrast IS the
                                 // volume read)
const ALBEDO: f32 = 0.97;
// Multi-scatter compensation on the sun term: a single-scatter march with a
// normalized phase (values ~0.04-0.1) can never reach a real cloud's
// brilliant white — higher scattering orders do that. Games boost the sun
// energy instead (Nubis's dual-lobe Beer is the same idea); without this the
// haze-colored ambient dominates and EVERY cloud reads gray.
const SUN_BOOST: f32 = 1.0;
const RAIN_DARKEN: f32 = 1.7;    // extinction boost at full menace — the
                                 // storm-slate knob. The darkening terms
                                 // COMPOUND (extinction × albedo bleed ×
                                 // in-scatter floor × ambient), so each is
                                 // kept gentle: a downpour base should read
                                 // deep slate (~1/3 sky luminance), never
                                 // near-black ("no harsh darks" — the art
                                 // direction; playtest 2026-07-17).
// Cloud fog-fade band: starts where the terrain fog completes (u.fog.y) and
// is COMPLETE by 3x that, so the deck dissolves into the haze exactly like
// terrain does — just over a cloud-scaled range (they are huge and high, so
// they legitimately outlive the terrain fog before melting away). PURELY
// proportional to the view distance: hard floors here (400/+600, removed
// 2026-07-19) pinned the band below ~25 chunks, so lowering the view
// distance fogged the terrain in but left clouds crisp to 400 blocks and
// visible to a kilometer.
const CLOUD_FADE_END_CAP: f32 = 2200.0;
const AMBIENT_LIFT: f32 = 0.56;  // how much haze light fills the shadow side
// Global opacity ceiling: a whisper of sky always shows through the deck —
// clouds read airy, never like a solid painted lid (per Rachel).
const CLOUD_SKY_BLEND: f32 = 0.1;
// Coverage keyframe spacing along the march (blocks) — must stay well under
// the field's smallest feature (~128 blocks: the 512-sheet's third octave).
const COV_KEY_SPACING: f32 = 40.0;
// Segment skip: below this keyed coverage nothing can render
// (cloud_density needs cov > ~0.18; the margin covers between-key peaks).
const COV_SKIP: f32 = 0.10;
// Beyond this march distance the billow erosion is skipped (cheap density):
// the aerial fade toward haze owns the look out there.
const BILLOW_LOD_T: f32 = 1200.0;
// Top rim light: skylight scattered down through thin cloud above the
// sample. DELIBERATELY sun-independent (per Rachel: bright crowns across
// the whole deck, not only toward the sun) and NOT menace-boosted — a storm
// cell keeps its slate body but still catches a lit crown, which is what
// keeps a busy sky from reading as one gloomy lid.
const RIM_BOOST: f32 = 0.7;
// Sun crown: direct daylight striking the deck's VERY top, in any view
// direction (the sun is overhead-ish all day — per Rachel, the tippity top
// is sunlit whenever it's daytime). Rides the rim visibility SQUARED so it
// hugs the crest tighter than the broad sky rim, and RAMPS UP with menace:
// fair-weather tops are already sunlit through the normal scatter path, but
// the menace extinction boost + albedo bleed steal exactly this light from
// storm cells, leaving their crests as dark as their bases (screenshot
// 2026-07-17) — physically a storm cloud's crown is its BRIGHTEST part.
const CROWN_SUN: f32 = 1.4;

// --- weather-core twins ----------------------------------------------------
fn fmix32(h_in: u32) -> u32 {
    var h = h_in;
    h ^= h >> 16u; h *= 0x85EBCA6Bu;
    h ^= h >> 13u; h *= 0xC2B2AE35u;
    h ^= h >> 16u;
    return h;
}

fn corner(ix: u32, iz: u32, seed: u32) -> f32 {
    let h = fmix32(ix * 0x9E3779B9u ^ iz * 0x85EBCA6Bu ^ seed);
    return f32(h >> 8u) / 16777216.0;
}

fn vnoise2(px: f32, pz: f32, period: u32, seed: u32) -> f32 {
    let fx = floor(px);
    let fz = floor(pz);
    let tx = smoothstep(0.0, 1.0, px - fx);
    let tz = smoothstep(0.0, 1.0, pz - fz);
    let mask = period - 1u;
    let ix = u32(i32(fx)) & mask; // period is a power of two; wraps negatives
    let iz = u32(i32(fz)) & mask;
    let x1 = (ix + 1u) & mask;
    let z1 = (iz + 1u) & mask;
    let a = corner(ix, iz, seed);
    let b = corner(x1, iz, seed);
    let c = corner(ix, z1, seed);
    let d = corner(x1, z1, seed);
    return mix(mix(a, b, tx), mix(c, d, tx), tz);
}

// One epoch seeding of the fbm — weather-core's twin: the middle octave
// advects at 2x the wind (INTEGER multiple, wrap-exact), so structure
// shears through the larger shapes instead of riding them rigidly.
fn fbm_epoch(q: vec2<f32>, o: vec2<f32>, base: u32, seed: u32) -> f32 {
    let n0 = vnoise2(q.x - o.x, q.y - o.y, base, seed);
    let n1 = vnoise2(
        (q.x - 2.0 * o.x) * 2.0,
        (q.y - 2.0 * o.y) * 2.0,
        base * 2u,
        seed ^ 0x9E3779B9u,
    );
    let n2 = vnoise2((q.x - o.x) * 4.0, (q.y - o.y) * 4.0, base * 4u, seed ^ 0x3C6EF372u);
    return (n0 + 0.5 * n1 + 0.25 * n2) / 1.75;
}

// Sheet B: weather-core's SHEET_B_* twins. Larger features advected at 2x
// the wind (INTEGER multiple — wrap-exactness); feature size a power of two
// dividing WRAP.
const SHEET_B_FEATURE: f32 = 1024.0;
const SHEET_B_ADVECT: f32 = 2.0;
const SHEET_B_SALT: u32 = 0x517CC1B7u;

// One cloud sheet — weather-core's `sheet` twin: epoch-morphed fbm (shapes
// REFORM while they drift; a rigid translation read as unnaturally uniform,
// playtest 2026-07-17) remapped by the storm bias to [0,1] coverage.
fn sheet_at(xz: vec2<f32>, salt: u32, feature: f32, advect: f32) -> f32 {
    let wind = params.values[0];
    let sky = params.values[1];
    let flux = params.values[2];
    let seed = u32(sky.w);
    let q = xz / feature; // lattice units; wrap via the period mask
    let o = advect * wind.xy / feature;
    let base = u32(WRAP / feature);
    let epoch = u32(flux.x);
    let n = mix(
        fbm_epoch(q, o, base, seed ^ fmix32(epoch) ^ salt),
        fbm_epoch(q, o, base, seed ^ fmix32(epoch + 1u) ^ salt),
        clamp(flux.y, 0.0, 1.0),
    );
    let lo = 1.0 - sky.x; // storm widens the covered fraction
    return clamp((n - lo) / max(1.0 - lo, 0.001), 0.0, 1.0);
}

// Coverage in [0,1] at world xz — the field the sim rains from: the
// SATURATING SUM of two sheets sliding at different speeds. Each sheet
// alone is thin fair-weather cloud; where they align the sum climbs through
// the rain band — fronts form by convergence (weather-core `coverage` twin).
fn coverage_at(xz: vec2<f32>) -> f32 {
    let ca = sheet_at(xz, 0u, params.values[1].z, 1.0);
    let cb = sheet_at(xz, SHEET_B_SALT, SHEET_B_FEATURE, SHEET_B_ADVECT);
    return clamp(ca + cb, 0.0, 1.0);
}

// How THREATENING a cell reads: 0 = fair-weather white, 1 = storm slate.
// Ramps up a little before the rain threshold (an approaching front already
// looms) and saturates where the downpour does, so color tells the player
// what the sky is about to do.
fn menace_at(cov: f32) -> f32 {
    // Opens just under the rain threshold: fair-weather clouds stay WHITE,
    // and gray is reserved for cells genuinely about to rain (0.6 was too
    // eager — with visible cloud from cov ~0.2, near-everything grayed).
    let rain_start = params.values[1].y;
    let full = rain_start + (1.0 - rain_start) * 0.6;
    return smoothstep(rain_start * 0.85, full, cov);
}

fn remap(v: f32, l0: f32, h0: f32, l1: f32, h1: f32) -> f32 {
    return l1 + (v - l0) * (h1 - l1) / (h0 - l0);
}

// The DIRECTIONAL haze color distant things dissolve into — mirror of the
// horizon formula in daynight_sky.wgsl / atmosphere.wgsl (chroma-boosted
// biome fog, warmed toward the sun; keep the three in sync). Fading toward
// the RAW fog color made far clouds go blue over a savanna while the
// terrain beneath melted into warm cream (playtest 2026-07-17).
fn haze_color(view_dir: vec3<f32>) -> vec3<f32> {
    let daylight = u.sun_dir.w;
    let toward = max(dot(view_dir, u.sun_dir.xyz), 0.0);
    let fog_luma = dot(u.fog_color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    var haze = (u.fog_color.rgb
        + (u.fog_color.rgb - vec3<f32>(fog_luma)) * vec3<f32>(0.0, 0.12, 0.55)) * 1.04;
    haze /= max(1.0, max(haze.r, max(haze.g, haze.b)));
    let glow = pow(toward, 8.0) * daylight * 0.70;
    return mix(haze, haze * vec3<f32>(1.22, 1.04, 0.84), glow);
}

fn corner3(ix: u32, iy: u32, iz: u32, seed: u32) -> f32 {
    return corner(ix ^ fmix32(iy * 0xC2B2AE35u), iz, seed);
}

fn vnoise3(p: vec3<f32>, period: u32, seed: u32) -> f32 {
    let f = floor(p);
    let t = smoothstep(vec3<f32>(0.0), vec3<f32>(1.0), p - f);
    let mask = period - 1u;
    let ix = u32(i32(f.x)) & mask;
    let iy = u32(i32(f.y)) & mask;
    let iz = u32(i32(f.z)) & mask;
    let x1 = (ix + 1u) & mask;
    let y1 = (iy + 1u) & mask;
    let z1 = (iz + 1u) & mask;
    let a = mix(corner3(ix, iy, iz, seed), corner3(x1, iy, iz, seed), t.x);
    let b = mix(corner3(ix, iy, z1, seed), corner3(x1, iy, z1, seed), t.x);
    let c = mix(corner3(ix, y1, iz, seed), corner3(x1, y1, iz, seed), t.x);
    let d = mix(corner3(ix, y1, z1, seed), corner3(x1, y1, z1, seed), t.x);
    return mix(mix(a, b, t.z), mix(c, d, t.z), t.y);
}

// Cloud density at a world point — the Nubis recipe scaled to a thin deck:
// the gameplay coverage field says WHERE cloud lives and how tall it towers;
// two octaves of wind-advected 3D billow noise carve that column into puffy
// cauliflower volumes (erode edges, keep cores); a height profile keeps
// bases flat-ish and tops domed.
fn cloud_density(p: vec3<f32>, cov: f32, cheap: bool) -> f32 {
    if (cov <= 0.02) { return 0.0; }
    let hn = clamp((p.y - CLOUD_BASE) / CLOUD_THICK, 0.0, 1.0);
    // Heavier cells tower higher; every cell keeps a flat-ish base band.
    let top = mix(0.4, 1.0, cov);
    let prof = smoothstep(0.0, 0.08, hn) * (1.0 - smoothstep(top * 0.45, top, hn));
    let base = clamp(remap(cov * prof, 0.18, 0.95, 0.0, 1.0), 0.0, 1.0);
    if (base <= 0.0) { return 0.0; }
    if (cheap) { return base * (0.45 + 0.55 * cov); }
    let wind = params.values[0];
    let sky = params.values[1];
    let seed = u32(sky.w);
    // Two billow octaves, advected with the field so shapes travel with
    // their cell. WRAP-EXACTNESS IS LOAD-BEARING: the horizontal scales
    // (64/16) divide WRAP and the advection is the RAW offset, so when the
    // published offset wraps, the lattice shifts by an exact period multiple
    // (65536/64 = 1024 = the mask; 65536/16 = 4096) and nothing pops. The y
    // axis has its own scale (puffs read rounder than tall) and never wraps.
    let adv = vec3<f32>(wind.x, 0.0, wind.y);
    let q1 = (p - adv) / vec3<f32>(64.0, 34.0, 64.0);
    let q2 = (p - adv) / vec3<f32>(16.0, 13.0, 16.0);
    let billow = 0.68 * vnoise3(q1, 1024u, seed ^ 0xA511E9B3u)
        + 0.32 * vnoise3(q2, 4096u, seed ^ 0x63D83595u);
    // Edge erosion, core-preserving (Nubis): thin edges dissolve into
    // cauliflower lobes, thick cores stay solid.
    let d = clamp(remap(base, (1.0 - billow) * 0.62 * (1.0 - base * 0.7), 1.0, 0.0, 1.0), 0.0, 1.0);
    return d * (0.45 + 0.55 * cov);
}

fn hg(mu: f32, g: f32) -> f32 {
    let gg = g * g;
    // pow(b, 1.5) == b*sqrt(b); b = (1-g)² at worst, always positive here.
    let b = 1.0 + gg - 2.0 * g * mu;
    return (1.0 - gg) / (12.566371 * b * sqrt(b));
}

// A small ordered dither so a 20-step march reads as soft layers, not bands
// (art direction: no per-pixel noise — this is a stable 4×4 Bayer).
fn bayer4(p: vec2<u32>) -> f32 {
    let m = array<f32, 16>(
        0.0, 8.0, 2.0, 10.0,
        12.0, 4.0, 14.0, 6.0,
        3.0, 11.0, 1.0, 9.0,
        15.0, 7.0, 13.0, 5.0,
    );
    return m[(p.y % 4u) * 4u + (p.x % 4u)] / 16.0;
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_env(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle at the far plane (the vs_sky convention).
    var out: VsOut;
    let x = f32(i32(vi) / 2) * 4.0 - 1.0;
    let y = f32(i32(vi) % 2) * 4.0 - 1.0;
    out.pos = vec4<f32>(x, y, 1.0, 1.0);
    out.ndc = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_env(in: VsOut) -> @location(0) vec4<f32> {
    if (u.fog.w > 0.5) { return vec4<f32>(0.0); } // underwater: no sky at all
    // Params not published (the engine normally skips the whole pass then;
    // this is defense in depth): a zero feature size would divide the
    // lattice math into NaNs.
    if (params.values[1].z <= 0.0) { return vec4<f32>(0.0); }

    // View ray + scene distance, both in render-local space (world direction;
    // world position = render_origin + local).
    let near_h = u.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far_h = u.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let near_p = near_h.xyz / near_h.w;
    let far_p = far_h.xyz / far_h.w;
    let dir = normalize(far_p - near_p);
    let cam_world = u.render_origin.xyz + u.cam_pos.xyz;

    let pixel = vec2<u32>(in.pos.xy);
    let scene_depth = textureLoad(depth_tex, vec2<i32>(pixel), 0);
    var scene_dist = CLOUD_FAR;
    if (scene_depth < 1.0) {
        let sh = u.inv_view_proj * vec4<f32>(in.ndc, scene_depth, 1.0);
        scene_dist = length(sh.xyz / sh.w - u.cam_pos.xyz);
    }

    // Analytic slab entry/exit on the view ray.
    var t0 = 0.0;
    var t1 = CLOUD_FAR;
    if (abs(dir.y) < 1e-4) {
        if (cam_world.y < CLOUD_BASE || cam_world.y > CLOUD_BASE + CLOUD_THICK) {
            return vec4<f32>(0.0);
        }
    } else {
        let ta = (CLOUD_BASE - cam_world.y) / dir.y;
        let tb = (CLOUD_BASE + CLOUD_THICK - cam_world.y) / dir.y;
        t0 = min(ta, tb);
        t1 = max(ta, tb);
    }
    t0 = max(t0, 0.0);
    t1 = min(t1, min(scene_dist, CLOUD_FAR));
    // Sub-millimeter spans (slab entry grazing the terrain clamp) render
    // nothing AND their dt can fall below the float ulp at large t — see the
    // step budget below.
    if (t1 <= t0 + 1e-3) { return vec4<f32>(0.0); }

    // Aerial fade band, terrain-style: from where the terrain fog completes
    // to a cloud-scaled 3x (see CLOUD_FADE_END_CAP). Computed before the
    // march so a ray entering the slab beyond full fade skips it entirely —
    // alpha would multiply to zero anyway (hit_dist >= t0 >= fade_end).
    let fade_start = u.fog.y;
    let fade_end = min(CLOUD_FADE_END_CAP, u.fog.y * 3.0);
    if (t0 >= fade_end) { return vec4<f32>(0.0); }

    let mu = dot(dir, u.sun_dir.xyz);
    let daylight = u.sun_dir.w;
    // Dual-lobe phase: soft forward scattering + a restrained silver lining.
    let phase = max(hg(mu, 0.55), 0.5 * hg(mu, 0.8));
    // Sun energy warms at the horizon (mirrors the sky's twilight warmth).
    let horizon_warm = clamp(1.0 - u.sun_dir.y * 2.2, 0.0, 1.0);
    let sun_color = mix(vec3<f32>(1.0, 0.985, 0.94), vec3<f32>(1.0, 0.75, 0.52), horizon_warm)
        * mix(0.06, 1.0, daylight); // moonlit decks stay dimly readable
    // Ambient fills from the luminous haze, tinted by the sim's sky lanes.
    // Its color leans toward WHITE sunlight for fair-weather cells (computed
    // per step by menace below) so wisps read bright, not haze-gray.
    let ambient_base = u.fog_color.rgb * u.sky_color.rgb * AMBIENT_LIFT;
    let ambient_white = vec3<f32>(1.0, 1.0, 1.0) * mix(0.12, 0.72, daylight);

    // COVERAGE KEYFRAMES: the coverage field is 2D with >=128-block features,
    // while the march samples every ~6-120 blocks — evaluating the full
    // two-sheet lattice per step (and per sun tap) was ~80% of the pass.
    // The march runs SEGMENT BY SEGMENT: coverage is evaluated once per
    // <=COV_KEY_SPACING-block key (lazily — an early transmittance break
    // skips the tail keys) and lerped across each segment; the 3D billow
    // (per step, as before) carries all the fine detail. A segment whose
    // BOTH keys sit under COV_SKIP is stepped over without any march body —
    // cloud_density needs cov > ~0.18 before anything renders, so clear sky
    // and the gaps BETWEEN clouds cost only their keys. No keys array: a
    // stored-array variant spilled to scratch and taxed every invocation.
    let span = t1 - t0;
    let n_keys = clamp(i32(span / COV_KEY_SPACING) + 1, 1, STEPS);
    let dt = span / f32(STEPS);
    var t = t0 + dt * bayer4(pixel);
    var transmittance = 1.0;
    var radiance = vec3<f32>(0.0);
    var hit_dist = -1.0;
    var seg_hi = coverage_at((cam_world + dir * t0).xz);
    // HARD STEP BUDGET: the march may take at most STEPS lit steps in total,
    // like the pre-segment for-loop it replaced. The while below is
    // FLOAT-conditioned; on a sliver span dt can be smaller than the ulp of
    // t, `t += dt` stops advancing, and without this cap that pixel loops
    // forever — one such pixel wedges the whole GPU channel (live Xid 109
    // CTX SWITCH TIMEOUT, 2026-07-17). Never remove the budget.
    var steps_left = STEPS;

    for (var k = 0; k < n_keys; k++) {
        let seg_t0 = t0 + span * f32(k) / f32(n_keys);
        let seg_t1 = t0 + span * f32(k + 1) / f32(n_keys);
        let seg_lo = seg_hi;
        seg_hi = coverage_at((cam_world + dir * seg_t1).xz);
        if (max(seg_lo, seg_hi) < COV_SKIP) {
            // Step past the segment on the same dt grid (keeps the dither
            // phase), paying nothing but the key eval.
            t += dt * max(0.0, ceil((seg_t1 - t) / dt));
            continue;
        }
        while (t < seg_t1 && steps_left > 0) {
        steps_left -= 1;
        let p = cam_world + dir * t;
        let cov = mix(seg_lo, seg_hi, clamp((t - seg_t0) / (seg_t1 - seg_t0), 0.0, 1.0));
        // Beyond the LOD line the aerial haze already owns the look: skip
        // the billow erosion (cheap density) for the sample and its taps.
        let cheap_lod = t > BILLOW_LOD_T;
        let density = cloud_density(p, cov, cheap_lod);
        if (density > 0.003) {
            if (hit_dist < 0.0) { hit_dist = t; }
            let menace = menace_at(cov);
            let hn = clamp((p.y - CLOUD_BASE) / CLOUD_THICK, 0.0, 1.0);
            // Sun occlusion: two full-density taps up the sun ray. The lit
            // dome vs shadowed underbelly contrast is the strongest
            // volumetric cue this shader has.
            // Sun taps reuse the ray sample's coverage: their 12/34-block
            // offsets are far below the field's >=128-block feature scale
            // (the vertical rim taps already reuse it exactly), so the
            // occlusion detail comes from the billow, not from re-evaluating
            // the lattice.
            let sp1 = p + u.sun_dir.xyz * 12.0;
            let sp2 = p + u.sun_dir.xyz * 34.0;
            let s1 = cloud_density(sp1, cov, cheap_lod);
            let s2 = cloud_density(sp2, cov, true);
            let tau_sun = (s1 * 12.0 + s2 * 26.0) * SIGMA_T * (1.0 + RAIN_DARKEN * menace);
            // Dual-lobe Beer: the wide lobe keeps shadowed flanks readable.
            let t_sun = max(exp(-tau_sun), 0.5 * exp(-0.25 * tau_sun));
            // Top rim + sun crown: two VERTICAL occlusion taps toward the
            // open sky. The coverage field is 2D, so the column's `cov` is
            // reused exactly — these taps cost only billow noise. No menace
            // boost on the taps: storm crowns stay lit (see RIM_BOOST).
            let r1 = cloud_density(p + vec3<f32>(0.0, 14.0, 0.0), cov, cheap_lod);
            let r2 = cloud_density(p + vec3<f32>(0.0, 36.0, 0.0), cov, true);
            let rim = exp(-(r1 * 14.0 + r2 * 28.0) * SIGMA_T);
            let rim_light = vec3<f32>(1.0, 0.99, 0.96) * mix(0.10, 1.0, daylight) * (RIM_BOOST * rim);
            let crown = sun_color * (CROWN_SUN * daylight * rim * rim * mix(0.3, 1.0, menace));
            // In-scatter probability: flat dark bases, bright rounded tops —
            // and a raised floor on LOW-menace cells, so fair-weather wisps
            // read luminous white while storm cells keep their gloom.
            let inscp_floor = mix(0.34, 0.18, menace);
            let inscp = (inscp_floor + (1.0 - inscp_floor) * pow(density, remap(hn, 0.2, 0.85, 0.55, 1.8)))
                * mix(0.42, 1.0, smoothstep(0.02, 0.4, hn));
            // Ambient climbs with height too — undersides sit in their own
            // shadow even away from the sun — and whitens as menace drops.
            let amb = mix(ambient_white, ambient_base, menace * 0.85 + 0.15)
                * mix(0.68, 1.15, hn);
            // The white→slate gradient (per Rachel): menace boosts absorption
            // AND bleeds scattering away, so threatening cells darken toward
            // grey even on their ambient-lit flanks — accumulated cloud IS
            // darker cloud, and the color says rain before the rain does.
            let sigma_e = SIGMA_T * density * (1.0 + RAIN_DARKEN * menace);
            let sigma_s = ALBEDO * SIGMA_T * density * (1.0 - 0.22 * menace);
            // The crown adds OUTSIDE the in-scatter shaping and WITHOUT the
            // menace albedo bleed: it is a surface-light hack, not
            // in-scatter — the crest's samples are thin (low density, inscp
            // near its floor) and menace-bled, which is precisely what kept
            // storm tops dark. sigma_e still divides it in the integral, so
            // energy conservation per step is preserved.
            let src = (sun_color * (t_sun * phase * daylight * SUN_BOOST) + amb + rim_light) * inscp * sigma_s
                + crown * (ALBEDO * SIGMA_T * density);
            // Energy-conserving step integration (Hillaire/Frostbite).
            let tr = exp(-sigma_e * dt);
            radiance += transmittance * (src - src * tr) / max(sigma_e, 1e-5);
            transmittance *= tr;
        }
        t += dt;
        if (transmittance < 0.02) { break; }
        }
        if (transmittance < 0.02) { break; }
    }

    var alpha = (1.0 - transmittance) * (1.0 - CLOUD_SKY_BLEND);
    if (alpha <= 0.002) { return vec4<f32>(0.0); }
    // Aerial perspective, terrain-style: purely by DISTANCE (an angle-based
    // horizon merge cut white bands through NEARBY eye-level clouds) —
    // distance lightens, and a far deck ends as haze, never as a hard grey
    // edge. The band itself is computed above the march.
    let haze = smoothstep(fade_start, fade_end, max(hit_dist, 0.0));
    // Fade toward the DIRECTIONAL haze (savanna cream, sunset peach), with
    // the COLOR converging faster than the alpha thins: the far deck first
    // BECOMES haze — matching the terrain and the sky's horizon band — and
    // only then dissolves, so it never pops blue against a warm horizon.
    var color = mix(radiance / max(alpha, 1e-4), haze_color(dir), haze);
    alpha *= 1.0 - haze * haze;
    // March cap fade: the deck thins out rather than ending on a line.
    alpha *= 1.0 - smoothstep(CLOUD_FAR * 0.72, CLOUD_FAR, max(hit_dist, 0.0));
    return vec4<f32>(color * alpha, alpha);
}
