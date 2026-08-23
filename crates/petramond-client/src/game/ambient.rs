//! Camera-following ambient particle volumes — precipitation, and the
//! INTERIOR volumes (drifting motes, dust) the same derive serves once a
//! bundle opts out of the two outdoor assumptions: the per-column
//! precipitation ceiling (`kill`) and the full-skylight constant (`light`).
//!
//! An `ambient` bundle (see [`petramond_world::particle_emitters`]) is DERIVED, not
//! simulated: every frame, each active drive re-computes its particle set as
//! a pure function of `(bundle, slot, cycle, time)` around the local camera.
//! Nothing persists, nothing runs on the tick, nothing replicates — a drive
//! is per-client presentation state a client mod sets (`ClientAmbientSet`)
//! with an intensity the engine eases so field-driven weather never pops.
//!
//! Ground behavior comes from the world's precipitation ceiling (the topmost
//! movement-blocking or water cell per column): a particle whose fall passed
//! that height is not drawn, and — when the bundle asks — its hit shows a
//! short splash whose droplets are ALSO closed-form (a parametric arc from
//! the referenced burst bundle's launch data), so splashes need no particle
//! pool and land exactly where each drop died, roofs included.

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

use glam::Vec3;

use petramond::entity::hash01;
use petramond::world::World;
use petramond_world::particle_emitters::{
    AmbientHit, AmbientKill, AmbientLight, AmbientMotion, AmbientSpec, BurstSpec,
};

use super::presentation::{ParticleAtlas, ParticlePresentation};

/// Seconds for an intensity change to close ~63% of its gap (exponential
/// ease) — weather fades in and out instead of popping.
const EASE_SECONDS: f32 = 2.0;
/// Below this eased intensity a drive with target 0 is dropped.
const DEAD_INTENSITY: f32 = 0.005;
/// Splashes derive only within this horizontal distance of the camera.
const SPLASH_RADIUS_SQ: f32 = 16.0 * 16.0;
/// Max derived droplets per splash.
const SPLASH_DROPLETS: usize = 4;
/// Downward acceleration on splash droplets (blocks/s²) — mirrors the
/// simulated burst pool's gravity so a derived splash reads the same.
const SPLASH_GRAVITY: f32 = 12.0;
/// Precipitation only exists under open sky, so it carries full skylight and
/// lets the ordinary sky lanes darken it at night. A bundle that can sit
/// indoors asks for `light: "world"` instead.
const SKY_OPEN_LIGHT: u8 = 63;
/// Fraction of the fall over which a fresh particle fades in at the band top.
const EDGE_FADE: f32 = 0.08;

/// One activated volume: the client mod's latest target + the eased actual.
struct Drive {
    target: f32,
    intensity: f32,
    wind: [f32; 2],
    /// Integrated wind advection, wrapped into the bundle's box diameter —
    /// the volume drifts by the INTEGRAL of the (changing) wind, exactly like
    /// the weather field itself. Multiplying the live wind by absolute time
    /// would make a wind CHANGE displace positions by Δwind × session-age
    /// (a lurch that grows with uptime).
    adv: [f32; 2],
}

/// All active ambient drives for the local client, keyed by mod id then
/// bundle id — two mods driving the same bundle stay independent (they also
/// derive with a per-mod seed salt, so their volumes interleave instead of
/// overlaying). Presentation-owned; see the module docs.
#[derive(Default)]
pub struct AmbientDrives {
    drives: BTreeMap<String, BTreeMap<u8, Drive>>,
    /// The volume clock: accumulated from clamped frame deltas, so it stays
    /// CONTINUOUS across the app clock's hourly wrap (a raw wrapped time
    /// would teleport-reseed every particle once an hour). Derivation is
    /// f32, so cycle fractions quantize very gradually with session age —
    /// sub-frame motion steps from roughly a day of continuous uptime; the
    /// f64 accumulator only keeps the ERROR from compounding.
    clock: f64,
    last_time: Option<f32>,
    /// Per-collect column cache: kill height (cell top face, absent when the
    /// column blocks nothing) + column biome per world column, or `None` for
    /// unloaded columns.
    ceilings: ColumnInfoCache,
}

impl AmbientDrives {
    /// Set a drive's target intensity (clamped to `0..=1` — the derive's
    /// effective range; the ABI documents the cap) and wind. A target at or
    /// below the liveness floor retires the volume after its ease-out (a
    /// sub-floor target would otherwise park a zombie drive forever).
    pub fn set(&mut self, owner: &str, bundle: u8, intensity: f32, wind: [f32; 2]) {
        let target = if intensity <= DEAD_INTENSITY {
            0.0
        } else {
            intensity.clamp(0.0, 1.0)
        };
        if let Some(drive) = self.drives.get_mut(owner).and_then(|m| m.get_mut(&bundle)) {
            drive.target = target;
            drive.wind = wind;
            return;
        }
        if target > 0.0 {
            self.drives.entry(owner.to_owned()).or_default().insert(
                bundle,
                Drive {
                    target,
                    intensity: 0.0,
                    wind,
                    adv: [0.0, 0.0],
                },
            );
        }
    }

    /// Drop every drive immediately (session teardown: a new world or the
    /// title screen must never inherit the old session's precipitation).
    pub fn clear(&mut self) {
        self.drives.clear();
    }

    /// Ease every drive toward its target and append this frame's derived
    /// particle rows. `time` is the app's render clock (only its DELTAS are
    /// consumed — see `Self::clock`); `cam` is the camera position;
    /// `density` is the particles graphics option (0 = off, the option
    /// exists to shed exactly this cost).
    pub fn collect(
        &mut self,
        world: &World,
        cam: Vec3,
        time: f32,
        density: f32,
        out: &mut Vec<ParticlePresentation>,
    ) {
        let dt = (time - self.last_time.unwrap_or(time)).clamp(0.0, 0.25);
        self.last_time = Some(time);
        self.clock += dt as f64;
        let ease = 1.0 - (-dt / EASE_SECONDS).exp();
        for per_owner in self.drives.values_mut() {
            per_owner.retain(|_, d| {
                d.intensity += (d.target - d.intensity) * ease;
                d.target > 0.0 || d.intensity > DEAD_INTENSITY
            });
        }
        self.drives.retain(|_, per_owner| !per_owner.is_empty());
        if self.drives.is_empty() || density <= 0.0 {
            return;
        }
        let clock = self.clock as f32;
        self.ceilings.clear();
        for (owner, per_owner) in &mut self.drives {
            // FNV-1a over the mod id: two mods on one bundle interleave.
            let owner_salt = owner.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
            });
            for (bundle, drive) in per_owner.iter_mut() {
                if drive.intensity <= DEAD_INTENSITY {
                    continue;
                }
                let Some(def) = petramond_world::particle_emitters::def(*bundle) else {
                    continue;
                };
                let Some(spec) = def.ambient.as_ref() else {
                    continue;
                };
                let diameter = spec.radius * 2.0;
                drive.adv = [
                    (drive.adv[0] + drive.wind[0] * spec.drift_wind * dt).rem_euclid(diameter),
                    (drive.adv[1] + drive.wind[1] * spec.drift_wind * dt).rem_euclid(diameter),
                ];
                let splash = match &spec.hit {
                    AmbientHit::Die => None,
                    AmbientHit::Burst(key) => petramond_world::particle_emitters::by_key(key)
                        .and_then(|b| b.burst.as_ref()),
                };
                derive_volume(
                    spec,
                    splash,
                    owner_salt ^ (*bundle as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                    drive.intensity * density.clamp(0.0, 1.0),
                    drive.wind,
                    drive.adv,
                    world,
                    &mut self.ceilings,
                    cam,
                    clock,
                    out,
                );
            }
        }
    }
}

#[inline]
fn lerp_range(range: [f32; 2], t: f32) -> f32 {
    range[0] + (range[1] - range[0]) * t
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Wrap `v` into `[-half, half)` of a `d`-wide box (the NVIDIA rain trick:
/// world-anchored positions re-enter the camera box on the far side).
#[inline]
fn wrap_center(v: f32, d: f32) -> f32 {
    v.rem_euclid(d) - d * 0.5
}

/// The kill height (blocking cell's TOP face, `None` when the column blocks
/// nothing) plus the column BIOME, for the column containing world `(x, z)`,
/// cached per collect. `None` = the column is not loaded. The biome feeds the
/// bundle's per-column filter — the rain/snow divide at a border is exact —
/// and is readable independently of the kill height, since a bundle may
/// declare a filter without asking for a ceiling kill.
/// Per-world-column memo for [`column_info`]: the killing ceiling height (or
/// `None` where the column blocks nothing) and the column's biome, against
/// `None` for a column that is not loaded.
type ColumnInfoCache = FxHashMap<(i32, i32), Option<(Option<f32>, u8)>>;

fn column_info(
    world: &World,
    cache: &mut ColumnInfoCache,
    x: f32,
    z: f32,
) -> Option<(Option<f32>, u8)> {
    let key = (x.floor() as i32, z.floor() as i32);
    *cache.entry(key).or_insert_with(|| {
        let biome = world.biome_at_world(key.0, key.1)?;
        let kill = world
            .precipitation_ceiling_y(key.0, key.1)
            .map(|y| y as f32 + 1.0);
        Some((kill, biome))
    })
}

/// The kill height for a column that must have one — the precipitation path's
/// original contract (`None` for unloaded columns AND for columns that block
/// nothing).
fn column_ceiling(world: &World, cache: &mut ColumnInfoCache, x: f32, z: f32) -> Option<(f32, u8)> {
    let (kill, biome) = column_info(world, cache, x, z)?;
    Some((kill?, biome))
}

#[allow(clippy::too_many_arguments)]
fn derive_volume(
    spec: &AmbientSpec,
    splash: Option<&BurstSpec>,
    drive_seed: u64,
    intensity: f32,
    wind: [f32; 2],
    adv: [f32; 2],
    world: &World,
    ceilings: &mut ColumnInfoCache,
    cam: Vec3,
    time: f32,
    out: &mut Vec<ParticlePresentation>,
) {
    let count = ((spec.count_per_intensity * intensity).round() as u32).min(spec.max_count);
    let diameter = spec.radius * 2.0;
    let span = spec.height[0] + spec.height[1];
    let y_top = cam.y + spec.height[1];
    // A volume's vertical extent is the `height` BAND, never the horizontal
    // diameter: a cavern is far wider than it is tall, and wrapping Y over
    // `diameter` would park most of the budget in the rock above the ceiling.
    let volume = spec.motion == AmbientMotion::Volume;
    let band_mid = y_top - span * 0.5;
    let wind_x = wind[0] * spec.drift_wind;
    let wind_z = wind[1] * spec.drift_wind;
    // The splash-anchor correction below rewinds the advection by a bounded
    // age (≤ a couple of seconds), over which the wind is effectively
    // constant — unlike absolute time, this never amplifies wind changes.
    let (adv_x, adv_z) = (adv[0], adv[1]);
    let has_flutter = spec.flutter[0] != 0.0;
    for i in 0..count {
        let seed = drive_seed ^ (i as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03);
        let fall_speed = lerp_range(spec.fall_speed, hash01(seed ^ 0x01));
        let cycle = span / fall_speed;
        // The cycle phase and flutter-phase hashes are reused by the splash
        // anchors below — draw each once.
        let phase = hash01(seed ^ 0x02);
        let (fphase_x, fphase_z) = if has_flutter {
            (hash01(seed ^ 0x05), hash01(seed ^ 0x06))
        } else {
            (0.0, 0.0)
        };
        let cycle_pos = time / cycle + phase;
        let cycle_idx = cycle_pos.floor();
        let t_cycle = cycle_pos - cycle_idx;
        // Per-cycle reseed: each pass down the band lands in a fresh column,
        // so the volume never visibly repeats. A world-anchored volume must
        // NOT reseed — its column is part of the mote's world position, and
        // re-drawing it would teleport the mote sideways every time it fell
        // through the vertical wrap.
        let cseed = if volume {
            seed
        } else {
            seed ^ (cycle_idx as i64 as u64).wrapping_mul(0xA24B_AED4_963E_E407)
        };
        // World-anchored column, drifting with the wind, wrapped into the
        // camera box so the volume follows without dragging its contents.
        let base_x = hash01(cseed ^ 0x03) * diameter + adv_x;
        let base_z = hash01(cseed ^ 0x04) * diameter + adv_z;
        // Flutter is part of the drop's REAL position: applying it before
        // the disc/ceiling checks keeps fluttering flakes out of the walls
        // beside open columns (and inside the disc the tests assert).
        let (flutter_x, flutter_z) = if has_flutter {
            (
                spec.flutter[0]
                    * (std::f32::consts::TAU * (spec.flutter[1] * time + fphase_x)).sin(),
                spec.flutter[0]
                    * (std::f32::consts::TAU * (spec.flutter[1] * time + fphase_z)).cos(),
            )
        } else {
            (0.0, 0.0)
        };
        let x = cam.x + wrap_center(base_x - cam.x, diameter) + flutter_x;
        let z = cam.z + wrap_center(base_z - cam.z, diameter) + flutter_z;
        let (dx, dz) = (x - cam.x, z - cam.z);
        let dist_sq = dx * dx + dz * dz;
        // The MOST RECENT landing's splash, anchored where that drop
        // actually died: its position and kill column are FROZEN at the hit
        // time (wind keeps blowing, the crown stays put), and its window is
        // the droplets' own lifetimes — a landing near the cycle end plays
        // out into the next cycle instead of truncating.
        if let Some(burst) = splash {
            for back in [0.0f32, 1.0] {
                let idx = cycle_idx - back;
                let hseed = seed ^ (idx as i64 as u64).wrapping_mul(0xA24B_AED4_963E_E407);
                let hx_base = hash01(hseed ^ 0x03) * diameter;
                let hz_base = hash01(hseed ^ 0x04) * diameter;
                // Hit time solved against the hit column's own ceiling: the
                // column is wind-stationary WITHIN a cycle apart from drift,
                // so anchor the column at the cycle's hit and look its
                // ceiling up once.
                // First locate the column at the (approximate) hit time via
                // the current wrap — one fixed-point step is plenty at
                // ≤ 6 b/s wind over ≤ a couple of seconds.
                let t_hit_guess = (idx + 0.85 - phase) * cycle;
                let adv_hx = adv_x - wind_x * (time - t_hit_guess);
                let adv_hz = adv_z - wind_z * (time - t_hit_guess);
                let hx = cam.x + wrap_center(hx_base + adv_hx - cam.x, diameter);
                let hz = cam.z + wrap_center(hz_base + adv_hz - cam.z, diameter);
                let (hdx, hdz) = (hx - cam.x, hz - cam.z);
                if hdx * hdx + hdz * hdz > SPLASH_RADIUS_SQ {
                    continue;
                }
                let Some((kill_y, hit_biome)) = column_ceiling(world, ceilings, hx, hz) else {
                    continue;
                };
                if !petramond_world::particle_emitters::biome_allowed(&spec.biome_allow, hit_biome)
                {
                    continue;
                }
                if kill_y >= y_top || kill_y <= y_top - span {
                    continue; // the ground is outside this band: no landing
                }
                // One fixed-point refinement: re-anchor the position at the
                // EXACT hit time (the 0.85 guess can be most of a cycle off,
                // which at full wind is several blocks) and re-read that
                // column's ceiling so the crown sits where the drop died.
                let t_hit = (idx + (y_top - kill_y) / span - phase) * cycle;
                let hx = cam.x
                    + wrap_center(hx_base + adv_x - wind_x * (time - t_hit) - cam.x, diameter);
                let hz = cam.z
                    + wrap_center(hz_base + adv_z - wind_z * (time - t_hit) - cam.z, diameter);
                // Re-check the splash gate on the REFINED anchor: the
                // correction (or a wrap fold) can move it past the visible
                // disc or the splash radius.
                let (rdx, rdz) = (hx - cam.x, hz - cam.z);
                let gate_sq = SPLASH_RADIUS_SQ.min(spec.radius * spec.radius);
                if rdx * rdx + rdz * rdz > gate_sq {
                    continue;
                }
                let Some((kill_y, refined_biome)) = column_ceiling(world, ceilings, hx, hz) else {
                    continue;
                };
                // The refinement can land columns away: keep the biome
                // divide exact for crowns too.
                if !petramond_world::particle_emitters::biome_allowed(
                    &spec.biome_allow,
                    refined_biome,
                ) {
                    continue;
                }
                if kill_y >= y_top || kill_y <= y_top - span {
                    continue;
                }
                let t_hit = (idx + (y_top - kill_y) / span - phase) * cycle;
                let age = time - t_hit;
                if age >= 0.0 {
                    // The crown sits where the flake VISUALLY died: include
                    // its (deterministic) flutter evaluated at the hit time.
                    let (fh_x, fh_z) = if has_flutter {
                        (
                            spec.flutter[0]
                                * (std::f32::consts::TAU * (spec.flutter[1] * t_hit + fphase_x))
                                    .sin(),
                            spec.flutter[0]
                                * (std::f32::consts::TAU * (spec.flutter[1] * t_hit + fphase_z))
                                    .cos(),
                        )
                    } else {
                        (0.0, 0.0)
                    };
                    let light = match spec.light {
                        AmbientLight::Sky => {
                            (SKY_OPEN_LIGHT, petramond_world::light::BlockLight6::DARK)
                        }
                        AmbientLight::World => world.dynamic_light_at_world(
                            hx.floor() as i32,
                            kill_y.floor() as i32,
                            hz.floor() as i32,
                        ),
                    };
                    derive_splash(burst, hseed, hx + fh_x, kill_y, hz + fh_z, age, light, out);
                }
            }
        }
        if dist_sq > spec.radius * spec.radius {
            continue; // square → disc thinning
        }
        // A fall sweeps the band from the top; a volume sinks from a
        // world-anchored height and wraps back in at the top, which is what
        // keeps the field standing still while the player jumps through it.
        // `t_band` is the shared 0-at-the-top position the edge fade reads,
        // so the wrap is hidden by the same ramp that hides a fresh drop.
        let (y, t_band) = if volume {
            let base_y = hash01(seed ^ 0x0A) * span - fall_speed * time;
            let y = band_mid + wrap_center(base_y - band_mid, span);
            (y, (y_top - y) / span)
        } else {
            (y_top - t_cycle * span, t_cycle)
        };
        let (cx, cy, cz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        // An interior volume's equivalent of "do not rain under a roof":
        // a mote that would sit inside the wall is not drawn. Rejecting it
        // here also keeps it out of the shared cube budget — in a cavern
        // roughly a third of the band is rock.
        if spec.kill == AmbientKill::Interior && world.blocks_movement_at(cx, cy, cz) {
            continue;
        }
        if spec.kill == AmbientKill::Ceiling || spec.biome_allow.is_some() {
            // Unloaded column: show nothing rather than rain through
            // structures.
            let Some((kill_y, biome)) = column_info(world, ceilings, x, z) else {
                continue;
            };
            // The bundle's per-column biome filter: at a biome border, rain
            // and snow bundles draw their divide column-exactly.
            if !petramond_world::particle_emitters::biome_allowed(&spec.biome_allow, biome) {
                continue;
            }
            if spec.kill == AmbientKill::Ceiling {
                // A column that blocks nothing has no kill height, and the
                // precipitation contract is to show nothing there.
                let Some(kill_y) = kill_y else { continue };
                if y <= kill_y {
                    continue; // landed: the splash block above showed it
                }
            }
        }
        let mix = if spec.color_bias == 1.0 {
            hash01(seed ^ 0x07) // powf(x, 1.0) == x, and powf is not cheap
        } else {
            hash01(seed ^ 0x07).powf(spec.color_bias)
        };
        let mut alpha = lerp_range(spec.alpha, hash01(seed ^ 0x09));
        // Fade in at the band top and out at the band bottom so particles
        // never pop into view — the bottom fade only ever shows when the
        // ground lies below the band (a camera high in the air).
        alpha *= (t_band / EDGE_FADE).min(1.0);
        alpha *= ((1.0 - t_band) / EDGE_FADE).min(1.0);
        let (skylight, blocklight) = match spec.light {
            AmbientLight::Sky => (SKY_OPEN_LIGHT, petramond_world::light::BlockLight6::DARK),
            AmbientLight::World => world.dynamic_light_at_world(cx, cy, cz),
        };
        out.push(ParticlePresentation {
            atlas: ParticleAtlas::Solid,
            pos: Vec3::new(x, y, z),
            uv_min: [0.0, 0.0],
            uv_size: [0.0; 2],
            tint: mix3(spec.color[0], spec.color[1], mix),
            alpha,
            size: lerp_range(spec.size, hash01(seed ^ 0x08)),
            stretch: spec.stretch,
            skylight,
            blocklight,
        });
    }
}

/// Closed-form splash: a few droplets on parametric launch arcs from the
/// burst bundle's data, alive for their rolled lifetimes after the hit.
#[allow(clippy::too_many_arguments)]
fn derive_splash(
    burst: &BurstSpec,
    cseed: u64,
    x: f32,
    kill_y: f32,
    z: f32,
    age: f32,
    light: (u8, petramond_world::light::BlockLight6),
    out: &mut Vec<ParticlePresentation>,
) {
    if age < 0.0 {
        return;
    }
    let droplets = (burst.count_per_intensity.ceil() as usize).clamp(1, SPLASH_DROPLETS);
    for k in 0..droplets {
        let dseed = cseed ^ (k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let life = lerp_range(burst.lifetime, hash01(dseed ^ 0x11));
        if age >= life {
            continue;
        }
        let t = age / life;
        let up = lerp_range(burst.up_speed, hash01(dseed ^ 0x12));
        let radial = lerp_range(burst.radial_speed, hash01(dseed ^ 0x13));
        let angle = std::f32::consts::TAU * hash01(dseed ^ 0x14);
        let mix = hash01(dseed ^ 0x16).powf(burst.color_bias);
        out.push(ParticlePresentation {
            atlas: ParticleAtlas::Solid,
            pos: Vec3::new(
                x + angle.cos() * radial * age,
                (kill_y + up * age - 0.5 * SPLASH_GRAVITY * age * age).max(kill_y),
                z + angle.sin() * radial * age,
            ),
            uv_min: [0.0, 0.0],
            uv_size: [0.0; 2],
            tint: mix3(burst.color[0], burst.color[1], mix),
            alpha: 0.9 * (1.0 - t),
            size: lerp_range(burst.size, hash01(dseed ^ 0x15)) * (1.0 - 0.5 * t),
            stretch: 1.0,
            skylight: light.0,
            blocklight: light.1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rain_spec(hit: AmbientHit) -> AmbientSpec {
        AmbientSpec {
            count_per_intensity: 200.0,
            max_count: 400,
            radius: 16.0,
            height: [4.0, 20.0],
            fall_speed: [16.0, 22.0],
            drift_wind: 1.0,
            flutter: [0.0, 0.0],
            size: [0.04, 0.06],
            stretch: 6.0,
            alpha: [0.4, 0.7],
            color: [[0.5, 0.6, 0.7], [0.7, 0.8, 0.9]],
            color_bias: 1.0,
            hit,
            motion: AmbientMotion::Precipitation,
            kill: AmbientKill::Ceiling,
            light: AmbientLight::Sky,
            biomes: Vec::new(),
            exclude_biomes: Vec::new(),
            biome_allow: None,
        }
    }

    fn splash_spec() -> BurstSpec {
        BurstSpec {
            count_per_intensity: 3.0,
            max_count: 24,
            up_speed: [1.0, 2.0],
            radial_speed: [0.5, 1.5],
            lifetime: [0.3, 0.5],
            size: [0.04, 0.08],
            color: [[0.2, 0.3, 0.8], [0.5, 0.7, 1.0]],
            color_bias: 1.0,
            die_on_contact: true,
        }
    }

    /// The shared 3×3-chunk fixture: a stone floor at y=64 (top face 65),
    /// open sky above. The camera floats over the floor with the ground
    /// INSIDE the fall band (`below` = 4 → band bottom 64 < floor top 65),
    /// so drops really land.
    const CAM: Vec3 = Vec3::new(8.0, 68.0, 8.0);
    const FLOOR_TOP: f32 = 65.0;

    #[test]
    fn particles_stay_inside_the_volume_and_above_the_ground() {
        let world = petramond::world::testutil::flat_world();
        let spec = rain_spec(AmbientHit::Die);
        let mut out = Vec::new();
        let mut ceilings = FxHashMap::default();
        for step in 0..40 {
            let time = step as f32 * 0.05;
            out.clear();
            ceilings.clear();
            derive_volume(
                &spec,
                None,
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                CAM,
                time,
                &mut out,
            );
            assert!(!out.is_empty(), "an active volume derives particles");
            for p in &out {
                assert!(
                    p.pos.y > FLOOR_TOP - 0.001,
                    "nothing renders below the floor top (y={})",
                    p.pos.y
                );
                assert!(p.pos.y <= CAM.y + spec.height[1] + 0.001);
                let (dx, dz) = (p.pos.x - CAM.x, p.pos.z - CAM.z);
                assert!(
                    dx * dx + dz * dz <= (spec.radius + 0.001).powi(2),
                    "particles stay inside the radius disc"
                );
                assert_eq!(p.stretch, spec.stretch);
            }
        }
    }

    #[test]
    fn covered_camera_derives_nothing_below_the_roof() {
        use petramond_world::block::Block;
        use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
        // A floor at y=64 AND a roof at y=80; the camera stands between them.
        let mut world = petramond::world::World::new(0, 1);
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        c.set_block(x, 64, z, Block::Stone);
                        c.set_block(x, 80, z, Block::Stone);
                    }
                }
                world.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
            }
        }
        let spec = rain_spec(AmbientHit::Die);
        let mut out = Vec::new();
        let mut ceilings = FxHashMap::default();
        for step in 0..40 {
            out.clear();
            ceilings.clear();
            derive_volume(
                &spec,
                None,
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                CAM,
                step as f32 * 0.07,
                &mut out,
            );
            for p in &out {
                assert!(
                    p.pos.y > 81.0 - 0.001,
                    "under a roof nothing falls below its top (y={})",
                    p.pos.y
                );
            }
        }
    }

    #[test]
    fn splashes_appear_at_the_kill_height_shortly_after_hits() {
        let world = petramond::world::testutil::flat_world();
        let spec = rain_spec(AmbientHit::Burst("resolved-by-caller".into()));
        let splash = splash_spec();
        let mut splashes_seen = 0;
        let mut out = Vec::new();
        let mut ceilings = FxHashMap::default();
        for step in 0..200 {
            out.clear();
            ceilings.clear();
            derive_volume(
                &spec,
                Some(&splash),
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                CAM,
                step as f32 * 0.03,
                &mut out,
            );
            for p in &out {
                // Splash droplets are the only rows without the rain stretch.
                if p.stretch == 1.0 {
                    splashes_seen += 1;
                    assert!(
                        p.pos.y >= FLOOR_TOP - 0.001 && p.pos.y < FLOOR_TOP + 1.5,
                        "droplets arc just above the floor top (y={})",
                        p.pos.y
                    );
                }
            }
        }
        assert!(splashes_seen > 10, "steady rain shows steady splashes");
    }

    #[test]
    fn drives_ease_in_and_retire_after_easing_out() {
        let world = petramond::world::testutil::flat_world();
        let mut drives = AmbientDrives::default();
        drives.set("weather", 200, 1.0, [0.0, 0.0]);
        // Unknown bundle id: the drive exists but derives nothing — inert.
        let mut out = Vec::new();
        drives.collect(&world, CAM, 0.0, 1.0, &mut out);
        assert!(out.is_empty());
        // Easing math: intensity approaches the target.
        let d = drives
            .drives
            .get("weather")
            .and_then(|m| m.get(&200))
            .unwrap();
        assert!(d.intensity < 1.0);
        for step in 1..200 {
            out.clear();
            drives.collect(&world, CAM, step as f32 * 0.1, 1.0, &mut out);
        }
        let d = drives
            .drives
            .get("weather")
            .and_then(|m| m.get(&200))
            .unwrap();
        assert!(d.intensity > 0.95, "intensity converges on the target");
        // Zero target: eases out, then the drive retires.
        drives.set("weather", 200, 0.0, [0.0, 0.0]);
        for step in 200..400 {
            out.clear();
            drives.collect(&world, CAM, step as f32 * 0.1, 1.0, &mut out);
        }
        assert!(drives.drives.is_empty(), "a zeroed drive retires");
        // A fresh zero-target set never creates a drive.
        drives.set("weather", 200, 0.0, [0.0, 0.0]);
        assert!(drives.drives.is_empty());
    }

    /// The round-3 regression guard: precipitation advects by the INTEGRAL
    /// of the wind, so under constant wind a splash crown's anchor must stay
    /// PUT across frames (the rewind exactly cancels the integral), and all
    /// falling rows must keep the disc/band/floor invariants while the
    /// integral wraps the camera box repeatedly. A revert to
    /// `wind × absolute time` (the defect round 3 caught) drifts the anchors
    /// and fails the cross-frame equality below.
    #[test]
    fn windy_advection_keeps_invariants_and_splash_anchors_static() {
        let world = petramond::world::testutil::flat_world();
        let spec = rain_spec(AmbientHit::Burst("resolved-by-caller".into()));
        // Zero launch speeds: droplets sit exactly ON their anchor for their
        // whole lifetime, so cross-frame anchor equality is directly
        // observable from the rows.
        let splash = BurstSpec {
            up_speed: [0.0, 0.0],
            radial_speed: [0.0, 0.0],
            ..splash_spec()
        };
        let diameter = spec.radius * 2.0;
        let mut adv = [0.0f32, 0.0];
        let mut ceilings = FxHashMap::default();
        let mut prev: Vec<(i32, i32)> = Vec::new();
        let mut prev_falling: Vec<(f32, f32)> = Vec::new();
        let mut crown_frames = 0;
        let mut displacement_frames = 0;
        let dt = 1.0 / 60.0;
        for step in 0..2000 {
            // The wind CHANGES mid-run — the whole point: starting from
            // adv=0 under constant wind, `wind × absolute time` equals the
            // integral and a revert to the round-3 defect would pass. After
            // the flip they diverge (the defect displaces every position by
            // Δwind × elapsed time; the integral glides).
            let wind = if step < 1000 {
                [5.0f32, -4.0]
            } else {
                [-3.0f32, 6.0]
            };
            let time = step as f32 * dt;
            adv[0] = (adv[0] + wind[0] * dt).rem_euclid(diameter);
            adv[1] = (adv[1] + wind[1] * dt).rem_euclid(diameter);
            let mut out = Vec::new();
            ceilings.clear();
            derive_volume(
                &spec,
                Some(&splash),
                7,
                1.0,
                wind,
                adv,
                &world,
                &mut ceilings,
                CAM,
                time,
                &mut out,
            );
            let mut crowns: Vec<(i32, i32)> = Vec::new();
            let mut falling: Vec<(f32, f32)> = Vec::new();
            for p in &out {
                let (dx, dz) = (p.pos.x - CAM.x, p.pos.z - CAM.z);
                assert!(
                    dx * dx + dz * dz <= (spec.radius + 0.001).powi(2),
                    "row outside the disc under wind (step {step})"
                );
                assert!(p.pos.y > FLOOR_TOP - 0.001, "row below the floor");
                assert!(p.pos.y <= CAM.y + spec.height[1] + 0.001);
                if p.stretch == 1.0 {
                    // Quantize to catch drift far above f32 noise but far
                    // below one particle spacing.
                    crowns.push((
                        (p.pos.x * 64.0).round() as i32,
                        (p.pos.z * 64.0).round() as i32,
                    ));
                } else {
                    falling.push((p.pos.x, p.pos.z));
                }
            }
            // THE regression signature: `wind × absolute time` displaces
            // every position by Δwind × session-age the moment the wind
            // changes (~2.5 blocks/frame here at the flip, vs ≤ ~0.6 for
            // legitimate fall+wind+flutter motion). Nearest-neighbour median
            // displacement of the falling rows stays small under the
            // integral, always.
            if !prev_falling.is_empty() && falling.len() >= 20 {
                let mut moved: Vec<f32> = falling
                    .iter()
                    .take(40)
                    .map(|(x, z)| {
                        prev_falling
                            .iter()
                            .map(|(px, pz)| {
                                let (dx, dz) = (x - px, z - pz);
                                dx * dx + dz * dz
                            })
                            .fold(f32::INFINITY, f32::min)
                    })
                    .collect();
                moved.sort_by(f32::total_cmp);
                let median = moved[moved.len() / 2].sqrt();
                assert!(
                    median < 1.2,
                    "falling rows jumped {median} blocks in one frame (step {step}) — \
                     advection must be the wind INTEGRAL, never wind × absolute time"
                );
                displacement_frames += 1;
            }
            prev_falling = falling;
            let in_flip_window = (995..1075).contains(&step);
            if step > 10 && !in_flip_window && !prev.is_empty() && !crowns.is_empty() {
                crown_frames += 1;
                let stable = crowns.iter().filter(|c| prev.contains(c)).count();
                // Splash windows (~0.2-0.3 s) span many 60 fps frames, so
                // MOST crowns persist frame-to-frame at identical positions.
                assert!(
                    stable * 2 >= crowns.len(),
                    "crown anchors drift under wind (step {step}: {stable}/{})",
                    crowns.len()
                );
            }
            prev = crowns;
        }
        assert!(
            crown_frames > 200,
            "the stability assertion must not be vacuous ({crown_frames} crown frames)"
        );
        assert!(
            displacement_frames > 1500,
            "the displacement assertion must not be vacuous ({displacement_frames})"
        );
    }

    /// A 3×3-chunk box: stone floor at y=64, stone roof at y=80, air between.
    fn roofed_world() -> petramond::world::World {
        use petramond_world::block::Block;
        use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
        let mut world = petramond::world::World::new(0, 1);
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        c.set_block(x, 64, z, Block::Stone);
                        c.set_block(x, 80, z, Block::Stone);
                    }
                }
                world.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
            }
        }
        world
    }

    /// THE reason an interior volume exists. Every enclosed space in the world
    /// sits under some column's precipitation ceiling, so a `kill: "ceiling"`
    /// bundle derives NOTHING down there however it is tuned.
    /// `kill: "interior"` fills the band around the camera regardless, and
    /// still refuses the cells inside the rock.
    #[test]
    fn an_interior_volume_derives_where_precipitation_cannot() {
        let world = roofed_world();
        // The camera sits between the floor and the roof, the way a player
        // stands in a cave: the whole band is under the ceiling.
        let cam = Vec3::new(8.0, 70.0, 8.0);
        let mut spec = rain_spec(AmbientHit::Die);
        spec.height = [6.0, 6.0];
        let mut ceilings = FxHashMap::default();
        let mut counts = [0usize; 2];
        for (i, kill) in [AmbientKill::Ceiling, AmbientKill::Interior]
            .into_iter()
            .enumerate()
        {
            spec.kill = kill;
            for step in 0..20 {
                let mut out = Vec::new();
                ceilings.clear();
                derive_volume(
                    &spec,
                    None,
                    7,
                    1.0,
                    [0.0, 0.0],
                    [0.0, 0.0],
                    &world,
                    &mut ceilings,
                    cam,
                    step as f32 * 0.11,
                    &mut out,
                );
                counts[i] += out.len();
            }
        }
        assert_eq!(counts[0], 0, "precipitation cannot exist under a roof");
        assert!(
            counts[1] > 500,
            "an interior volume fills the band anyway, got {}",
            counts[1]
        );
        // …and it still refuses to draw inside the walls. The band's bottom
        // metre is the stone floor, so this is not a vacuous assertion.
        let mut out = Vec::new();
        ceilings.clear();
        derive_volume(
            &spec,
            None,
            7,
            1.0,
            [0.0, 0.0],
            [0.0, 0.0],
            &world,
            &mut ceilings,
            cam,
            3.0,
            &mut out,
        );
        assert!(
            out.iter().all(|p| p.pos.y >= 65.0),
            "no mote inside the floor"
        );
        assert!(
            out.len() < (spec.count_per_intensity * 0.79) as usize,
            "the floor must actually reject motes ({} of {})",
            out.len(),
            spec.count_per_intensity
        );
    }

    /// The lighting knob, which is what stops an interior volume from being a
    /// field of glowing dots in a pitch-dark cave: `light: "world"` samples
    /// the cell, `light: "sky"` keeps the precipitation constant.
    #[test]
    fn world_lit_motes_take_the_cells_own_light() {
        use petramond_world::chunk::SECTION_VOLUME;
        use petramond_world::light::{BlockLight6, LightRgb};
        let mut world = roofed_world();
        // Bake the band the camera stands in DARK, then flood ONE chunk's
        // section with a coloured emitter's light. A constant would pass a
        // dark-only assertion; only a positional sample can tell the two
        // halves of this fixture apart.
        let lamp = LightRgb::new(28, 6, 24);
        for cz in -1..=1i32 {
            for cx in -1..=1i32 {
                let section = world
                    .section_at_world_mut_for_test(cx * 16, 70, cz * 16)
                    .expect("the band's section is loaded");
                section.set_skylight(vec![0u8; SECTION_VOLUME].into());
                if (cx, cz) == (0, 0) {
                    section.set_blocklight(vec![lamp; SECTION_VOLUME].into());
                }
            }
        }
        let cam = Vec3::new(8.0, 70.0, 8.0);
        let mut spec = rain_spec(AmbientHit::Die);
        spec.height = [6.0, 6.0];
        spec.kill = AmbientKill::Interior;
        let sample = |spec: &AmbientSpec| {
            let mut out = Vec::new();
            let mut ceilings = FxHashMap::default();
            derive_volume(
                spec,
                None,
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                cam,
                1.0,
                &mut out,
            );
            assert!(!out.is_empty());
            out
        };
        spec.light = AmbientLight::Sky;
        for p in sample(&spec) {
            assert_eq!(p.skylight, SKY_OPEN_LIGHT);
            assert_eq!(p.blocklight, BlockLight6::DARK);
        }
        spec.light = AmbientLight::World;
        let (mut dark, mut lit) = (0usize, 0usize);
        for p in sample(&spec) {
            assert_eq!(p.skylight, 0, "a sealed room has no skylight");
            let in_lamp = (0.0..16.0).contains(&p.pos.x) && (0.0..16.0).contains(&p.pos.z);
            if in_lamp {
                assert_eq!(p.blocklight, BlockLight6::from_x2(lamp));
                lit += 1;
            } else {
                assert_eq!(
                    p.blocklight,
                    BlockLight6::DARK,
                    "a mote outside the lit chunk must stay dark"
                );
                dark += 1;
            }
        }
        assert!(
            lit > 20 && dark > 20,
            "both halves populated ({lit}/{dark})"
        );
    }

    /// THE anchoring contract, and the bug Rachel played into: a `volume`
    /// bundle's motes keep their WORLD positions while the camera moves —
    /// jumping must not carry the field along — yet the body still follows
    /// the player so it is always populated around them. A `precipitation`
    /// bundle deliberately keeps the old camera-relative band in Y.
    ///
    /// Invisible in a still frame, so this test is the only real proof.
    #[test]
    fn a_volume_is_world_anchored_while_precipitation_rides_the_camera() {
        // Open air well above the fixture's floor: every mote survives, so
        // the two frames hold the same motes and nothing else can explain a
        // difference.
        let world = petramond::world::testutil::flat_world();
        let base_cam = Vec3::new(8.0, 120.0, 8.0);
        let mut spec = rain_spec(AmbientHit::Die);
        spec.count_per_intensity = 400.0;
        spec.max_count = 400;
        spec.height = [8.0, 10.0];
        spec.fall_speed = [0.22, 0.5];
        spec.flutter = [0.55, 0.06];
        spec.kill = AmbientKill::Interior;
        spec.light = AmbientLight::World;
        let span = spec.height[0] + spec.height[1];
        let time = 37.0;
        let sample = |spec: &AmbientSpec, cam: Vec3| {
            let mut out = Vec::new();
            let mut ceilings = FxHashMap::default();
            derive_volume(
                spec,
                None,
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                cam,
                time,
                &mut out,
            );
            assert!(out.len() > 100, "the fixture must derive a real field");
            out.iter().map(|p| p.pos).collect::<Vec<_>>()
        };
        // Fraction of `b` that still sits at a position present in `a`,
        // identified by the two axes the camera did not move along.
        let anchored = |a: &[Vec3], b: &[Vec3], axis: usize| -> f32 {
            let others: Vec<usize> = (0..3).filter(|i| *i != axis).collect();
            let kept = b
                .iter()
                .filter(|p| {
                    a.iter().any(|q| {
                        others.iter().all(|i| (p[*i] - q[*i]).abs() < 1e-4)
                            && (p[axis] - q[axis]).abs() < 1e-3
                    })
                })
                .count();
            kept as f32 / b.len() as f32
        };

        for (motion, expect_anchored) in [
            (AmbientMotion::Volume, true),
            (AmbientMotion::Precipitation, false),
        ] {
            spec.motion = motion;
            let jump = 1.25; // roughly a player's jump apex
            let base = sample(&spec, base_cam);
            let jumped = sample(&spec, base_cam + Vec3::new(0.0, jump, 0.0));
            let held = anchored(&base, &jumped, 1);
            if expect_anchored {
                // Only the motes that wrapped past the band edge may move,
                // i.e. at most jump/span of them.
                assert!(
                    held > 1.0 - jump / span - 0.02,
                    "a volume must keep its motes' world height through a jump (kept {held})"
                );
            } else {
                assert!(
                    held < 0.05,
                    "precipitation keeps its camera-relative band (kept {held})"
                );
            }
            // Either way the body follows the player: still populated, still
            // inside the band and the disc around the NEW camera.
            let cam = base_cam + Vec3::new(0.0, jump, 0.0);
            for p in &jumped {
                assert!(
                    p.y >= cam.y - spec.height[0] - 1e-3 && p.y <= cam.y + spec.height[1] + 1e-3,
                    "mote outside the band around the new camera (y={})",
                    p.y
                );
                let (dx, dz) = (p.x - cam.x, p.z - cam.z);
                assert!(dx * dx + dz * dz <= (spec.radius + 1e-3).powi(2));
            }
            // Control: the HORIZONTAL anchor both kinds already had. If the
            // helper could not detect anchoring at all, this would fail too.
            let strafed = sample(&spec, base_cam + Vec3::new(3.0, 0.0, 0.0));
            let held_x = anchored(&base, &strafed, 0);
            // A 3-block strafe swaps ~12% of the disc for fresh motes, so
            // this is loose by construction; it is here to prove the helper
            // can SEE anchoring, not to measure it.
            assert!(
                held_x > 0.75,
                "{motion:?}: motes must keep their world X when the camera strafes ({held_x})"
            );
        }
    }

    /// A volume must be populated wherever the player is, including far from
    /// where it was last derived — the wrap, not a lattice, is what makes the
    /// world anchor affordable.
    #[test]
    fn a_teleported_camera_still_stands_in_a_full_volume() {
        let world = petramond::world::testutil::flat_world();
        let mut spec = rain_spec(AmbientHit::Die);
        spec.motion = AmbientMotion::Volume;
        spec.height = [8.0, 10.0];
        spec.kill = AmbientKill::Interior;
        spec.light = AmbientLight::World;
        let mut ceilings = FxHashMap::default();
        // All well above the fixture's floor: below it the world reads as
        // virtual stone and the interior kill would (correctly) empty the band.
        for cam_y in [120.0f32, 200.0, 90.0] {
            let mut out = Vec::new();
            ceilings.clear();
            derive_volume(
                &spec,
                None,
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                Vec3::new(8.0, cam_y, 8.0),
                101.0,
                &mut out,
            );
            assert!(
                out.len() > 100,
                "the volume follows to y={cam_y} ({} motes)",
                out.len()
            );
            for p in &out {
                assert!(p.pos.y >= cam_y - spec.height[0] - 1e-3);
                assert!(p.pos.y <= cam_y + spec.height[1] + 1e-3);
            }
        }
    }

    /// The per-column biome filter: a bundle whose allow-set excludes the
    /// fixture's biome derives NOTHING; the complement derives normally.
    #[test]
    fn biome_filter_gates_columns() {
        let world = petramond::world::testutil::flat_world();
        let mut allowed = rain_spec(AmbientHit::Die);
        let mut denied = rain_spec(AmbientHit::Die);
        // The fixture's columns default to biome 0.
        allowed.biome_allow = Some([1u64, 0, 0, 0]); // bit 0 set
        denied.biome_allow = Some([!1u64, u64::MAX, u64::MAX, u64::MAX]);
        let mut ceilings = FxHashMap::default();
        for (spec, expect_some) in [(&allowed, true), (&denied, false)] {
            let mut out = Vec::new();
            ceilings.clear();
            derive_volume(
                spec,
                None,
                7,
                1.0,
                [0.0, 0.0],
                [0.0, 0.0],
                &world,
                &mut ceilings,
                CAM,
                1.0,
                &mut out,
            );
            assert_eq!(
                !out.is_empty(),
                expect_some,
                "biome filter must gate the derive"
            );
        }
    }
}
