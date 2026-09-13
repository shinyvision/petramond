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
// Sculpted billows with a continuous three-tone palette: cool undersides,
// pearl bodies, cream/peach crowns. Coverage, rain and wind remain one field.

struct Uniforms {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog: vec4<f32>,        // (start, end, time, underwater)
    fog_color: vec4<f32>,  // rgb = haze color (night-dimmed); w = sky scale
    inv_view_proj: mat4x4<f32>,
    render_origin: vec4<i32>,
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
const CLOUD_THICK: f32 = 128.0;
const CLOUD_FAR: f32 = 2400.0;   // march cap; beyond, the deck fades to haze
// Density samples sit at most MAX_STEP_LENGTH blocks apart along the ray,
// never fewer than MIN_STEPS of them: a fixed budget spread over a grazing
// ray's kilometres is what showed the sample layers through the deck
// (banding fix, 2026-09-05). MAX_STEPS is not a tuning knob — it is exactly
// the count the longest possible span needs at that spacing (the march ends
// at CLOUD_FADE_END_CAP), kept as the integer bound that makes the
// float-conditioned loop below safe on sliver spans.
//
// Raising the budget is cheap here because the coverage keys are capped
// independently (MAX_COV_KEYS): in the earlier fixed-28-step march the key
// count rode the step count, so more steps re-exposed the high-frequency
// field the keys smooth, and 36 steps measured worse than 28. With the keys
// decoupled the cap binds only on grazing spans, where the clear-segment
// skip and the transmittance early-out already cut most of the work:
// measured against the fixed-28-step march, the environment chain costs a
// small, bounded amount more at its worst views and the same at the zenith,
// and coarser spacing saves a fraction of that while bringing the layers
// back.
const MIN_STEPS: i32 = 28;
const MAX_STEP_LENGTH: f32 = 12.0;
const WRAP: f32 = 65536.0;
const SIGMA_T: f32 = 0.11;       // extinction at density 1 — thick cores go
                                 // optically deep fast (that contrast IS the
                                 // volume read)
const RAIN_DARKEN: f32 = 1.25;

// Cloud fog-fade band: starts where the terrain fog completes (u.fog.y) and
// is COMPLETE by 3x that, so the deck dissolves into the haze exactly like
// terrain does — just over a cloud-scaled range (they are huge and high, so
// they legitimately outlive the terrain fog before melting away). PURELY
// proportional to the view distance: hard floors here (400/+600, removed
// 2026-07-19) pinned the band below ~25 chunks, so lowering the view
// distance fogged the terrain in but left clouds crisp to 400 blocks and
// visible to a kilometer.
const CLOUD_FADE_END_CAP: f32 = 2200.0;
const MAX_STEPS: i32 = i32(ceil(CLOUD_FADE_END_CAP / MAX_STEP_LENGTH));
// Global opacity ceiling: a whisper of sky always shows through the deck —
// clouds read airy, never like a solid painted lid (per Rachel).
const CLOUD_SKY_BLEND: f32 = 0.1;
// Coverage keyframe spacing along the march (blocks) — must stay well under
// the field's smallest feature (~128 blocks: the 512-sheet's third octave).
const COV_KEY_SPACING: f32 = 40.0;
// Keep field interpolation independent of the density sampling budget.
const MAX_COV_KEYS: i32 = 28;
// Segment skip: below this keyed coverage nothing can render
// (cloud_density needs cov > ~0.18; the margin covers between-key peaks).
const COV_SKIP: f32 = 0.10;
// Beyond this march distance the billow erosion fades out (cheap density):
// the aerial fade toward haze owns the look out there.
const BILLOW_LOD_T: f32 = 1200.0;
// Fade unresolved billow octaves by sample footprint before sampling noise.
const BILLOW_FINE_NYQ: f32 = 16.0;   // 32-block octave
const BILLOW_FINE_OUT: f32 = 40.0;
const BILLOW_COARSE_NYQ: f32 = 32.0; // 64-block lobes
const BILLOW_COARSE_OUT: f32 = 80.0;
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

// Coverage owns the cloud footprint; wind-advected billows sculpt its edges.
// Detail fades continuously toward the distance/footprint LOD.
fn cloud_density(p: vec3<f32>, cov: f32, detail: vec2<f32>) -> f32 {
    if (cov <= 0.02) { return 0.0; }
    let hn = clamp((p.y - CLOUD_BASE) / CLOUD_THICK, 0.0, 1.0);
    let top = mix(0.58, 1.0, cov);
    let crown = 1.0 - smoothstep(top * 0.68, top, hn);
    // Reject outside the fullest possible profile before paying for billows.
    if (cov * smoothstep(0.0, 0.14, hn) * crown <= 0.18) { return 0.0; }
    let wind = params.values[0];
    let sky = params.values[1];
    let seed = u32(sky.w);
    // Broad lobes carry the silhouette; small scallops only break its edge.
    // Horizontal wavelengths divide WRAP, so wrapped wind never jumps.
    let adv = vec3<f32>(wind.x, 0.0, wind.y);
    var billow = 1.0;
    if (detail.x > 0.01) {
        let q1 = (p - adv) / vec3<f32>(64.0, 56.0, 64.0);
        billow -= detail.x * 0.84 * (1.0 - smoothstep(0.18, 0.82, vnoise3(q1, 1024u, seed ^ 0xA511E9B3u)));
    }
    if (detail.y > 0.01) {
        let q2 = (p - adv) / vec3<f32>(32.0, 24.0, 32.0);
        billow -= detail.y * 0.16 * (1.0 - vnoise3(q2, 2048u, seed ^ 0x63D83595u));
    }
    // Dense lobes hang lower, with soft recesses between them. Reusing the
    // billows keeps the underside attached to the same wind-advected shape.
    let bottom = (1.0 - billow) * 0.22;
    let prof = smoothstep(bottom, bottom + 0.14, hn) * crown;
    let base = clamp(remap(cov * prof, 0.18, 0.88, 0.0, 1.0), 0.0, 1.0);
    // Edge erosion, core-preserving (Nubis): thin edges dissolve into
    // cauliflower lobes, thick cores stay solid.
    let d = clamp(remap(base, (1.0 - billow) * 0.66 * (1.0 - base * 0.7), 1.0, 0.0, 1.0), 0.0, 1.0);
    return d * (0.70 + 0.65 * cov);
}

fn cloud_palette(sun_visibility: f32, sky_visibility: f32, height: f32, menace: f32, mu: f32) -> vec3<f32> {
    let daylight = u.sun_dir.w;
    let sunset = clamp(1.0 - max(u.sun_dir.y, 0.0) * 2.2, 0.0, 1.0);
    let light = clamp(0.45 * sun_visibility + 0.45 * sky_visibility + 0.25 * height, 0.0, 1.0);
    let body = smoothstep(0.22, 0.64, light);
    let crest = smoothstep(0.62, 0.94, light) * smoothstep(0.04, 0.30, height);
    let shadow = mix(vec3<f32>(0.26, 0.33, 0.49), vec3<f32>(0.13, 0.17, 0.29), menace);
    let pearl = mix(vec3<f32>(0.62, 0.71, 0.85), vec3<f32>(0.43, 0.50, 0.66), menace);
    let cream = mix(vec3<f32>(0.94, 0.91, 0.82), vec3<f32>(1.0, 0.57, 0.34), sunset * 0.85);
    let day_color = mix(mix(shadow, pearl, body), cream, crest);
    let night_color = mix(vec3<f32>(0.012, 0.018, 0.036), vec3<f32>(0.065, 0.085, 0.14), body * 0.45 + crest * 0.55);
    // A narrow silver edge adds definition without whitening the whole bank.
    let lining = pow(max(mu, 0.0), 8.0) * sky_visibility * (1.0 - sky_visibility) * 0.28;
    return mix(night_color, day_color + cream * lining, daylight);
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
    // The origin enters WRAPPED: every lattice this pass samples tiles WRAP,
    // so dropping whole periods changes nothing and keeps the float small
    // however far out the camera is.
    let cam_world = vec3<f32>(
        f32(u.render_origin.x % i32(WRAP)),
        f32(u.render_origin.y),
        f32(u.render_origin.z % i32(WRAP)),
    ) + u.cam_pos.xyz;

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
    // Aerial fade band, terrain-style: from where the terrain fog completes
    // to a cloud-scaled 3x (see CLOUD_FADE_END_CAP). Computed before the
    // march so a ray entering the slab beyond full fade skips it entirely —
    // all cloud samples would lie beyond the fade.
    let fade_start = u.fog.y;
    let fade_end = min(CLOUD_FADE_END_CAP, u.fog.y * 3.0);
    t0 = max(t0, 0.0);
    if (t0 >= fade_end) { return vec4<f32>(0.0); }
    // The march ENDS at the fade too, not at the raw CLOUD_FAR cap: past
    // fade_end the deck is pure haze, so those samples only ever bought
    // opacity the haze then took back. Grazing rays are the ones this cuts
    // (their slab crossing runs kilometres), and cutting their span cuts dt
    // with it — which is what lets the billow LOD keep its detail out to a
    // much lower elevation angle.
    t1 = min(t1, min(scene_dist, min(CLOUD_FAR, fade_end)));
    // Sub-millimeter spans (slab entry grazing the terrain clamp) render
    // nothing AND their dt can fall below the float ulp at large t — see the
    // step budget below.
    if (t1 <= t0 + 1e-3) { return vec4<f32>(0.0); }

    let mu = dot(dir, u.sun_dir.xyz);
    // COVERAGE KEYFRAMES: the coverage field is 2D with >=128-block features,
    // while the march resolves the smaller billows — evaluating the full
    // two-sheet lattice per step (and per sun tap) was ~80% of the pass.
    // Coverage keys are lazy and independently capped, so early termination
    // skips tail keys and denser sampling does not change field interpolation.
    // Coverage is lerped across each segment; the 3D billow
    // (per step, as before) carries all the fine detail. A segment whose
    // BOTH keys sit under COV_SKIP is stepped over without any march body —
    // cloud_density needs cov > ~0.18 before anything renders, so clear sky
    // and the gaps BETWEEN clouds cost only their keys. No keys array: a
    // stored-array variant spilled to scratch and taxed every invocation.
    let span = t1 - t0;
    let n_keys = clamp(i32(span / COV_KEY_SPACING) + 1, 1, MAX_COV_KEYS);
    let sample_count = clamp(i32(ceil(span / MAX_STEP_LENGTH)), MIN_STEPS, MAX_STEPS);
    let dt = span / f32(sample_count);
    // Footprint LOD, constant along the march (dt is): each octave's weight
    // at this step spacing. The old hard `t > BILLOW_LOD_T` switch folds in
    // per step below as a distance factor, so the far deck now eases into
    // cheap density instead of popping at 1200.
    let detail_dt = vec2<f32>(
        1.0 - smoothstep(BILLOW_COARSE_NYQ, BILLOW_COARSE_OUT, dt),
        1.0 - smoothstep(BILLOW_FINE_NYQ, BILLOW_FINE_OUT, dt),
    );
    var t = t0 + dt * 0.5;
    var transmittance = 1.0;
    var radiance = vec3<f32>(0.0);
    var weighted_dist = 0.0;
    var seg_hi = coverage_at((cam_world + dir * t0).xz);
    // Float increments can stop advancing on sliver spans at large distances.
    // Keep the integer cap even with the span guard, or one ray can hang the GPU.
    var steps_left = sample_count;

    for (var k = 0; k < n_keys; k++) {
        let seg_t0 = t0 + span * f32(k) / f32(n_keys);
        let seg_t1 = t0 + span * f32(k + 1) / f32(n_keys);
        let seg_lo = seg_hi;
        seg_hi = coverage_at((cam_world + dir * seg_t1).xz);
        if (max(seg_lo, seg_hi) < COV_SKIP) {
            // Preserve midpoint spacing across clear segments.
            t += dt * max(0.0, ceil((seg_t1 - t) / dt));
            continue;
        }
        while (t < seg_t1 && steps_left > 0) {
            steps_left -= 1;
            let p = cam_world + dir * t;
            let cov = mix(seg_lo, seg_hi, clamp((t - seg_t0) / (seg_t1 - seg_t0), 0.0, 1.0));
            // Toward the LOD line the aerial haze already owns the look: the
            // billow erosion fades out (cheap density) for the sample and its
            // taps.
            let detail = detail_dt * (1.0 - smoothstep(BILLOW_LOD_T * 0.75, BILLOW_LOD_T, t));
            let density = cloud_density(p, cov, detail);
            if (density > 0.003) {
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
                let s1 = cloud_density(sp1, cov, detail);
                let s2 = cloud_density(sp2, cov, vec2<f32>(0.0));
                let tau_sun = (s1 * 12.0 + s2 * 26.0) * SIGMA_T * (1.0 + RAIN_DARKEN * menace);
                // Dual-lobe Beer: the wide lobe keeps shadowed flanks readable.
                let t_sun = max(exp(-tau_sun), 0.5 * exp(-0.25 * tau_sun));
                // Top rim + sun crown: two VERTICAL occlusion taps toward the
                // open sky. The coverage field is 2D, so the column's `cov` is
                // reused exactly — these taps cost only billow noise. No menace
                // boost on the taps: storm crowns stay lit.
                let r1 = cloud_density(p + vec3<f32>(0.0, 14.0, 0.0), cov, detail);
                let r2 = cloud_density(p + vec3<f32>(0.0, 36.0, 0.0), cov, vec2<f32>(0.0));
                let rim = exp(-(r1 * 14.0 + r2 * 28.0) * SIGMA_T);
                let sigma_e = SIGMA_T * density * (1.0 + RAIN_DARKEN * menace);
                let cloud_color = cloud_palette(t_sun, rim, hn, menace, mu);
                // Integrate the lit colour over exactly the opacity this step adds.
                let tr = exp(-sigma_e * dt);
                let weight = transmittance * (1.0 - tr);
                weighted_dist += weight * t;
                radiance += weight * cloud_color;
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
    // First-hit distance jumps a whole step when a density threshold is crossed.
    // Opacity-weighted distance follows the visible cloud continuously instead.
    let mean_dist = weighted_dist / max(1.0 - transmittance, 1e-4);
    let haze = smoothstep(fade_start, fade_end, mean_dist);
    // Fade toward the DIRECTIONAL haze (savanna cream, sunset peach), with
    // the COLOR converging faster than the alpha thins: the far deck first
    // BECOMES haze — matching the terrain and the sky's horizon band — and
    // only then dissolves, so it never pops blue against a warm horizon.
    var color = mix(radiance / max(1.0 - transmittance, 1e-4), haze_color(dir), haze);
    alpha *= 1.0 - haze * haze;
    // March cap fade: the deck thins out rather than ending on a line.
    alpha *= 1.0 - smoothstep(CLOUD_FAR * 0.72, CLOUD_FAR, mean_dist);
    return vec4<f32>(color * alpha, alpha);
}
