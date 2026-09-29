//! The FLIGHT motion kind: a lattice of world-anchored fliers (butterflies,
//! and whatever a pack authors next) orbiting closed-form paths above the
//! ground. Like every ambient kind it is derived per frame from
//! `(bundle, cell, time)` — nothing is simulated or replicated.
//!
//! Admission is decided for the WHOLE orbit, never for the flier's current
//! column: every column the orbit (plus wing reach) can occupy must be loaded,
//! carry one of the row's ground tags at its precipitation ceiling, pass the
//! bundle's biome filter, and admit the flier's density roll. The cruise
//! height is the HIGHEST such ground, so every phase clears a slope; a canopy
//! or a roof over any part of the orbit refuses the flier outright rather than
//! lifting it. This is what keeps a flier from blinking off over uphill steps
//! or re-rolling its existence at a biome border.

use std::f32::consts::{FRAC_1_SQRT_2, TAU};

use glam::Vec3;
use petramond::entity::hash01;
use petramond::world::ReplicaWorld;
use petramond_world::particle_emitters::{AmbientLight, AmbientSpec, FlightSpec};

use super::super::presentation::{ParticleAtlas, ParticlePresentation};
use super::{
    colour, column_facts, floor_i32, lerp_range, Activation, ColumnInfoCache, View, SKY_OPEN_LIGHT,
};

const HOVER_BOB_RATE: f32 = 2.3;
const ORBIT_Z_RATE: f32 = 0.73;
const WING_FOLD: [f32; 2] = [0.15, 1.30];
const WING_SPAN: f32 = 0.25;
const WING_LENGTH: f32 = 0.5;
const RADIUS_FADE: f32 = 4.0;
const BAND_FADE: f32 = 2.0;

pub(super) fn orbit(flight: &FlightSpec, seed: u64, time: f32) -> (Vec3, Vec3) {
    let phase = hash01(seed ^ 3) * TAU;
    let t = time * lerp_range(flight.speed, hash01(seed ^ 4)) + phase;
    let [ox, oz] = flight.orbit;
    let offset = Vec3::new(
        ox * t.sin(),
        flight.hover[0] + flight.hover[1] * (t * HOVER_BOB_RATE).sin(),
        oz * (t * ORBIT_Z_RATE).cos(),
    );
    let heading = Vec3::new(
        ox * t.cos(),
        0.0,
        -oz * ORBIT_Z_RATE * (t * ORBIT_Z_RATE).sin(),
    );
    (offset, heading.normalize_or(Vec3::Z))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn orbit_ground(
    spec: &AmbientSpec,
    flight: &FlightSpec,
    act: &Activation,
    world: &ReplicaWorld,
    columns: &mut ColumnInfoCache,
    x: f64,
    z: f64,
    roll: f32,
) -> Option<f32> {
    let reach = spec.size[1] * FRAC_1_SQRT_2;
    let (rx, rz) = (flight.orbit[0] + reach, flight.orbit[1] + reach);
    let mut highest = f32::NEG_INFINITY;
    let (rx, rz) = (f64::from(rx), f64::from(rz));
    for wz in floor_i32(z - rz)..=floor_i32(z + rz) {
        for wx in floor_i32(x - rx)..=floor_i32(x + rx) {
            let facts = column_facts(world, columns, wx, wz)?;
            let biome = facts.biome;
            if !petramond_world::particle_emitters::biome_allowed(&spec.biome_allow, biome)
                || !act.admits(roll, biome)
            {
                return None;
            }
            let ground = facts.ceiling?;
            let support = facts.support?;
            if !flight.ground.is_empty() && !flight.ground.iter().any(|&t| support.has_tag(t)) {
                return None;
            }
            highest = highest.max(ground as f32 + 1.0);
        }
    }
    Some(highest)
}

#[inline]
pub(super) fn cell_seed(act: &Activation, gx: i32, gz: i32) -> u64 {
    act.seed
        ^ (gx as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ (gz as u64).wrapping_mul(0xA24B_AED4_963E_E407)
}

#[inline]
pub(super) fn cell_anchor(flight: &FlightSpec, seed: u64, gx: i32, gz: i32) -> (f64, f64) {
    let spacing = f64::from(flight.spacing);
    (
        (f64::from(gx) + f64::from(hash01(seed ^ 1))) * spacing,
        (f64::from(gz) + f64::from(hash01(seed ^ 2))) * spacing,
    )
}

pub(super) fn derive_flight(
    spec: &AmbientSpec,
    flight: &FlightSpec,
    act: &Activation,
    view: &View,
    columns: &mut ColumnInfoCache,
    out: &mut Vec<ParticlePresentation>,
) {
    if act.intensity <= 0.0 {
        return;
    }
    let (world, cam, time) = (view.world, view.cam, view.time);
    let sprite = flight.sprite_tile.map(petramond_render::atlas::tile_uv);
    let cx = (cam.x / f64::from(flight.spacing)).floor() as i32;
    let cz = (cam.z / f64::from(flight.spacing)).floor() as i32;
    let reach =
        ((spec.radius + flight.orbit[0].max(flight.orbit[1])) / flight.spacing).ceil() as i32;
    for gz in cz - reach..=cz + reach {
        for gx in cx - reach..=cx + reach {
            let seed = cell_seed(act, gx, gz);
            let (x, z) = cell_anchor(flight, seed, gx, gz);
            let (offset, heading) = orbit(flight, seed, time);
            let (px, pz) = (x + f64::from(offset.x), z + f64::from(offset.z));
            let distance = (px - cam.x).hypot(pz - cam.z) as f32;
            if distance >= spec.radius {
                continue;
            }
            let roll = hash01(seed ^ 5) / (act.intensity * flight.occupancy);
            if roll >= 1.0 {
                continue;
            }
            let Some(ground) = orbit_ground(spec, flight, act, world, columns, x, z, roll) else {
                continue;
            };
            let pos =
                petramond_math::world_pos::WorldPos::new(px, f64::from(ground + offset.y), pz);
            let dy = (pos.y - cam.y) as f32;
            if dy < -spec.height[0] || dy > spec.height[1] {
                continue;
            }
            let band_edge = if dy < 0.0 {
                spec.height[0] + dy
            } else {
                spec.height[1] - dy
            };
            let fade = ((spec.radius - distance) / RADIUS_FADE).clamp(0.0, 1.0)
                * (band_edge / BAND_FADE).clamp(0.0, 1.0);
            let size = lerp_range(spec.size, hash01(seed ^ 6)) * fade;
            let alpha = lerp_range(spec.alpha, hash01(seed ^ 0x0A));
            let tint = colour(spec, seed ^ 9);
            let (skylight, blocklight) = match spec.light {
                AmbientLight::Sky => (SKY_OPEN_LIGHT, petramond_world::light::BlockLight6::DARK),
                AmbientLight::World => world.data().dynamic_light_at_world(
                    px.floor() as i32,
                    pos.y.floor() as i32,
                    pz.floor() as i32,
                ),
            };
            let Some([u0, v0, u1, v1]) = sprite else {
                out.push(ParticlePresentation {
                    atlas: ParticleAtlas::Solid,
                    quad_axes: None,
                    pos,
                    uv_min: [0.0, 0.0],
                    uv_size: [0.0; 2],
                    tint,
                    alpha,
                    size,
                    stretch: 1.0,
                    skylight,
                    blocklight,
                });
                continue;
            };
            let beat = TAU * lerp_range(flight.flap_hz, hash01(seed ^ 7));
            let fold = lerp_range(
                WING_FOLD,
                0.5 + 0.5 * (time * beat + hash01(seed ^ 8) * TAU).sin(),
            );
            let right = Vec3::new(heading.z, 0.0, -heading.x);
            for side in [-1.0, 1.0] {
                let wing = (right * side * fold.cos() + Vec3::Y * fold.sin()) * size * WING_SPAN;
                out.push(ParticlePresentation {
                    atlas: ParticleAtlas::Block,
                    quad_axes: Some([wing * side, heading * size * WING_LENGTH]),
                    pos: pos + wing,
                    uv_min: [if side < 0.0 { u0 } else { (u0 + u1) * 0.5 }, v0],
                    uv_size: [(u1 - u0) * 0.5, v1 - v0],
                    tint,
                    alpha,
                    size,
                    stretch: 1.0,
                    skylight,
                    blocklight,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
