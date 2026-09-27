//! World-anchored ambient particles around the local camera: precipitation,
//! the INTERIOR volumes (drifting motes, dust) the same derive serves once a
//! bundle opts out of the two outdoor assumptions — the per-column
//! precipitation ceiling (`kill`) and the full-skylight constant (`light`) —
//! and FLIGHTS ([`flight`]): lattices of fliers orbiting above the ground.
//!
//! An `ambient` bundle (see [`petramond_world::particle_emitters`]) is DERIVED, not
//! simulated: every frame, each active bundle re-computes its particle set as
//! a pure function of `(bundle, slot, cycle, time)` around the local camera.
//! Nothing persists, nothing runs on the tick, nothing replicates. A bundle is
//! active either through a DRIVE — per-client presentation state a client mod
//! sets (`ClientAmbientSet`) with an intensity the engine eases so
//! field-driven weather never pops — or because a biome row lists it under
//! `ambient` with a density, in which case every client derives it wherever
//! the local columns' biomes give it one (`biome_intensity`), thinning each
//! particle by its own column's density.
//!
//! Ground behavior comes from the world's precipitation ceiling (the topmost
//! movement-blocking or water cell per column): a particle whose fall passed
//! that height is not drawn, and — when the bundle asks — its hit shows a
//! short splash whose droplets are ALSO closed-form (a parametric arc from
//! the referenced burst bundle's launch data), so splashes need no particle
//! pool and land exactly where each drop died, roofs included.

use std::collections::BTreeMap;

mod flight;

use rustc_hash::FxHashMap;

use glam::Vec3;

use petramond::entity::hash01;
use petramond::world::ReplicaWorld;
use petramond_world::particle_emitters::{
    self, AmbientHit, AmbientKill, AmbientLight, AmbientMotion, AmbientSpec, BurstSpec,
};

use super::presentation::{ParticleAtlas, ParticlePresentation};

const EASE_SECONDS: f32 = 2.0;
const DEAD_INTENSITY: f32 = 0.005;
const SPLASH_RADIUS_SQ: f32 = 16.0 * 16.0;
const SPLASH_DROPLETS: usize = 4;
const SPLASH_GRAVITY: f32 = 12.0;
const SKY_OPEN_LIGHT: u8 = 63;
const EDGE_FADE: f32 = 0.08;

/// One activated volume: the client mod's latest target + the eased actual.
struct Drive {
    target: f32,
    intensity: f32,
    wind: [f32; 2],
    /// Wind drift summed over time and wrapped to the bundle's box diameter. Using wind * age
    /// instead would make the volume jump every time the wind changes, worse the longer you play.
    adv: [f32; 2],
}

#[derive(Default)]
pub struct AmbientDrives {
    drives: BTreeMap<String, BTreeMap<u8, Drive>>,
    clock: f64,
    last_time: Option<f32>,
    ceilings: ColumnInfoCache,
}

impl AmbientDrives {
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

    pub fn clear(&mut self) {
        self.drives.clear();
    }

    pub fn collect(
        &mut self,
        world: &ReplicaWorld,
        cam: petramond_math::world_pos::WorldPos,
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
        if density <= 0.0 {
            return;
        }
        let density = density.clamp(0.0, 1.0);
        let view = View {
            world,
            cam,
            time: self.clock as f32,
        };
        self.ceilings.clear();
        for (owner, per_owner) in &mut self.drives {
            let owner_salt = owner.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
            });
            for (bundle, drive) in per_owner.iter_mut() {
                if drive.intensity <= DEAD_INTENSITY {
                    continue;
                }
                let Some(spec) = particle_emitters::def(*bundle).and_then(|d| d.ambient.as_ref())
                else {
                    continue;
                };
                let diameter = spec.radius * 2.0;
                drive.adv = [
                    (drive.adv[0] + drive.wind[0] * spec.drift_wind * dt).rem_euclid(diameter),
                    (drive.adv[1] + drive.wind[1] * spec.drift_wind * dt).rem_euclid(diameter),
                ];
                let bundle = *bundle;
                let weight = |biome| particle_emitters::biome_intensity(bundle, biome);
                derive(
                    spec,
                    &Activation {
                        seed: owner_salt ^ bundle_salt(bundle),
                        intensity: drive.intensity * density,
                        wind: drive.wind,
                        adv: drive.adv,
                        biome_weight: particle_emitters::biome_driven()
                            .contains(&bundle)
                            .then_some(&weight as &dyn Fn(u8) -> f32),
                    },
                    &view,
                    &mut self.ceilings,
                    out,
                );
            }
        }
        for &bundle in particle_emitters::biome_driven() {
            let Some(spec) = particle_emitters::def(bundle).and_then(|d| d.ambient.as_ref()) else {
                continue;
            };
            derive(
                spec,
                &Activation {
                    seed: bundle_salt(bundle),
                    intensity: density,
                    wind: [0.0, 0.0],
                    adv: [0.0, 0.0],
                    biome_weight: Some(&|biome| particle_emitters::biome_intensity(bundle, biome)),
                },
                &view,
                &mut self.ceilings,
                out,
            );
        }
    }
}

#[inline]
fn bundle_salt(bundle: u8) -> u64 {
    (bundle as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

struct Activation<'a> {
    seed: u64,
    intensity: f32,
    wind: [f32; 2],
    adv: [f32; 2],
    biome_weight: Option<&'a dyn Fn(u8) -> f32>,
}

impl Activation<'_> {
    #[inline]
    fn admits(&self, roll: f32, biome: u8) -> bool {
        self.biome_weight.is_none_or(|weight| roll < weight(biome))
    }
}

struct View<'a> {
    world: &'a ReplicaWorld,
    cam: petramond_math::world_pos::WorldPos,
    time: f32,
}

fn derive(
    spec: &AmbientSpec,
    act: &Activation,
    view: &View,
    ceilings: &mut ColumnInfoCache,
    out: &mut Vec<ParticlePresentation>,
) {
    match &spec.motion {
        AmbientMotion::Flight(flight) => flight::derive_flight(spec, flight, act, view, out),
        AmbientMotion::Precipitation | AmbientMotion::Volume => {
            let splash = match &spec.hit {
                AmbientHit::Die => None,
                AmbientHit::Burst(key) => {
                    particle_emitters::by_key(key).and_then(|b| b.burst.as_ref())
                }
            };
            derive_volume(spec, splash, act, view, ceilings, out);
        }
    }
}

fn colour(spec: &AmbientSpec, seed: u64) -> [f32; 3] {
    if spec.palette.is_empty() {
        let mix = if spec.color_bias == 1.0 {
            hash01(seed)
        } else {
            hash01(seed).powf(spec.color_bias)
        };
        return mix3(spec.color[0], spec.color[1], mix);
    }
    let total: f32 = spec.palette.iter().map(|stop| stop.weight).sum();
    let mut roll = hash01(seed) * total;
    for stop in &spec.palette {
        if roll < stop.weight {
            return stop.color;
        }
        roll -= stop.weight;
    }
    spec.palette[spec.palette.len() - 1].color
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

#[inline]
fn wrap_center(v: f32, d: f32) -> f32 {
    v.rem_euclid(d) - d * 0.5
}

#[inline]
fn wrap_offset(anchor: f32, cam: f64, d: f32) -> f32 {
    ((f64::from(anchor) - cam).rem_euclid(f64::from(d)) - f64::from(d) * 0.5) as f32
}

type ColumnInfoCache = FxHashMap<(i32, i32), Option<(Option<f32>, u8)>>;

fn column_info(
    world: &ReplicaWorld,
    cache: &mut ColumnInfoCache,
    x: f64,
    z: f64,
) -> Option<(Option<f32>, u8)> {
    let key = (x.floor() as i32, z.floor() as i32);
    *cache.entry(key).or_insert_with(|| {
        let biome = world.data().biome_at_world(key.0, key.1)?;
        let kill = world
            .data()
            .precipitation_ceiling_y(key.0, key.1)
            .map(|y| y as f32 + 1.0);
        Some((kill, biome))
    })
}

fn column_ceiling(
    world: &ReplicaWorld,
    cache: &mut ColumnInfoCache,
    x: f64,
    z: f64,
) -> Option<(f32, u8)> {
    let (kill, biome) = column_info(world, cache, x, z)?;
    Some((kill?, biome))
}

fn derive_volume(
    spec: &AmbientSpec,
    splash: Option<&BurstSpec>,
    act: &Activation,
    view: &View,
    ceilings: &mut ColumnInfoCache,
    out: &mut Vec<ParticlePresentation>,
) {
    let (world, cam, time) = (view.world, view.cam, view.time);
    let (drive_seed, intensity, wind, adv) = (act.seed, act.intensity, act.wind, act.adv);
    let count = ((spec.count_per_intensity * intensity).round() as u32).min(spec.max_count);
    let diameter = spec.radius * 2.0;
    let span = spec.height[0] + spec.height[1];
    let y_top = cam.y as f32 + spec.height[1];
    let volume = matches!(spec.motion, AmbientMotion::Volume);
    let band_mid = y_top - span * 0.5;
    let wind_x = wind[0] * spec.drift_wind;
    let wind_z = wind[1] * spec.drift_wind;
    let (adv_x, adv_z) = (adv[0], adv[1]);
    let has_flutter = spec.flutter[0] != 0.0;
    let biome_gated = act.biome_weight.is_some();
    for i in 0..count {
        let seed = drive_seed ^ (i as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03);
        let fall_speed = lerp_range(spec.fall_speed, hash01(seed ^ 0x01));
        let cycle = span / fall_speed;
        let phase = hash01(seed ^ 0x02);
        let biome_roll = hash01(seed ^ 0x0B);
        let (fphase_x, fphase_z) = if has_flutter {
            (hash01(seed ^ 0x05), hash01(seed ^ 0x06))
        } else {
            (0.0, 0.0)
        };
        let band_phase = phase + if volume { 0.0 } else { y_top / span };
        let cycle_pos = time / cycle + band_phase;
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
        let base_x = hash01(cseed ^ 0x03) * diameter + adv_x;
        let base_z = hash01(cseed ^ 0x04) * diameter + adv_z;
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
        let dx = wrap_offset(base_x, cam.x, diameter) + flutter_x;
        let dz = wrap_offset(base_z, cam.z, diameter) + flutter_z;
        let (x, z) = (cam.x + f64::from(dx), cam.z + f64::from(dz));
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
                let t_hit_guess = (idx + 0.85 - band_phase) * cycle;
                let adv_hx = adv_x - wind_x * (time - t_hit_guess);
                let adv_hz = adv_z - wind_z * (time - t_hit_guess);
                let hdx = wrap_offset(hx_base + adv_hx, cam.x, diameter);
                let hdz = wrap_offset(hz_base + adv_hz, cam.z, diameter);
                let (hx, hz) = (cam.x + f64::from(hdx), cam.z + f64::from(hdz));
                if hdx * hdx + hdz * hdz > SPLASH_RADIUS_SQ {
                    continue;
                }
                let Some((kill_y, hit_biome)) = column_ceiling(world, ceilings, hx, hz) else {
                    continue;
                };
                if !particle_emitters::biome_allowed(&spec.biome_allow, hit_biome)
                    || !act.admits(biome_roll, hit_biome)
                {
                    continue;
                }
                if kill_y >= y_top || kill_y <= y_top - span {
                    continue;
                }
                let t_hit = (idx - kill_y / span - phase) * cycle;
                let rdx = wrap_offset(hx_base + adv_x - wind_x * (time - t_hit), cam.x, diameter);
                let rdz = wrap_offset(hz_base + adv_z - wind_z * (time - t_hit), cam.z, diameter);
                let (hx, hz) = (cam.x + f64::from(rdx), cam.z + f64::from(rdz));
                let gate_sq = SPLASH_RADIUS_SQ.min(spec.radius * spec.radius);
                if rdx * rdx + rdz * rdz > gate_sq {
                    continue;
                }
                let Some((kill_y, refined_biome)) = column_ceiling(world, ceilings, hx, hz) else {
                    continue;
                };
                if !particle_emitters::biome_allowed(&spec.biome_allow, refined_biome)
                    || !act.admits(biome_roll, refined_biome)
                {
                    continue;
                }
                if kill_y >= y_top || kill_y <= y_top - span {
                    continue;
                }
                let t_hit = (idx - kill_y / span - phase) * cycle;
                let age = time - t_hit;
                if age >= 0.0 {
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
                        AmbientLight::World => world.data().dynamic_light_at_world(
                            hx.floor() as i32,
                            kill_y.floor() as i32,
                            hz.floor() as i32,
                        ),
                    };
                    let (sx, sz) = (hx + f64::from(fh_x), hz + f64::from(fh_z));
                    derive_splash(burst, hseed, sx, kill_y, sz, age, light, out);
                }
            }
        }
        if dist_sq > spec.radius * spec.radius {
            continue;
        }
        // Falls sweep the band from the top. Volumes instead sink from a height pinned to the
        // world and wrap back in at the top, which is why the field doesn't move when you jump
        // through it. The edge fade reads `t_band` (0 at the top), so the wrap hides behind the
        // same ramp as a fresh drop.
        let (y, t_band) = if volume {
            let base_y = hash01(seed ^ 0x0A) * span - fall_speed * time;
            let y = band_mid + wrap_center(base_y - band_mid, span);
            (y, (y_top - y) / span)
        } else {
            (y_top - t_cycle * span, t_cycle)
        };
        let (cx, cy, cz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        if spec.kill == AmbientKill::Interior && world.data().blocks_movement_at(cx, cy, cz) {
            continue;
        }
        if spec.kill == AmbientKill::Ceiling || spec.biome_allow.is_some() || biome_gated {
            let Some((kill_y, biome)) = column_info(world, ceilings, x, z) else {
                continue;
            };
            if !particle_emitters::biome_allowed(&spec.biome_allow, biome)
                || !act.admits(biome_roll, biome)
            {
                continue;
            }
            if spec.kill == AmbientKill::Ceiling {
                let Some(kill_y) = kill_y else { continue };
                if y <= kill_y {
                    continue;
                }
            }
        }
        let mut alpha = lerp_range(spec.alpha, hash01(seed ^ 0x09));
        alpha *= (t_band / EDGE_FADE).min(1.0);
        alpha *= ((1.0 - t_band) / EDGE_FADE).min(1.0);
        let (skylight, blocklight) = match spec.light {
            AmbientLight::Sky => (SKY_OPEN_LIGHT, petramond_world::light::BlockLight6::DARK),
            AmbientLight::World => world.data().dynamic_light_at_world(cx, cy, cz),
        };
        out.push(ParticlePresentation {
            quad_axes: None,
            atlas: ParticleAtlas::Solid,
            pos: petramond_math::world_pos::WorldPos::new(x, f64::from(y), z),
            uv_min: [0.0, 0.0],
            uv_size: [0.0; 2],
            tint: colour(spec, seed ^ 0x07),
            alpha,
            size: lerp_range(spec.size, hash01(seed ^ 0x08)),
            stretch: spec.stretch,
            skylight,
            blocklight,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_splash(
    burst: &BurstSpec,
    cseed: u64,
    x: f64,
    kill_y: f32,
    z: f64,
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
            quad_axes: None,
            atlas: ParticleAtlas::Solid,
            pos: petramond_math::world_pos::WorldPos::new(x, f64::from(kill_y), z)
                + Vec3::new(
                    angle.cos() * radial * age,
                    (up * age - 0.5 * SPLASH_GRAVITY * age * age).max(0.0),
                    angle.sin() * radial * age,
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
mod tests;
