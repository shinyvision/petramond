//! Tiny 3D particle cubes.
//!
//! Each [`ParticleInstance`] draws as a small textured cube rather than a billboard, so dust is
//! visible from any angle, including straight above. Every face samples the particle's patch of
//! the block atlas, times its tint and a per-face shade.
//!
//! The CPU writes one [`ParticleRow`] per particle (80 bytes) and nothing per vertex. The
//! `particles.wgsl` vertex stage expands each instance into 24 cube vertices from `vertex_index`
//! and the `wgsl_faces` table. The fragment uses an alpha cutout, so cubes are depth-tested and
//! depth-written: terrain occludes them and they sort among themselves. Dying particles shrink.
//!
//! Block-row emitters reuse the row format for solid-colour cubes on a separate alpha-blended
//! pipeline. Those are sorted far-to-near and back-face culled, so small transparent flames don't
//! show all six faces at once.
//!
//! Instances are unbounded; the buffer grows to fit (`DynamicInstanceDraw`).

use super::lighting::{self, DynLight, LightEnv};
use super::{ParticleEmitterInstance, ParticleInstance};
use glam::Vec3;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleRow {
    pub center: [f32; 3],
    pub half: f32,
    pub right: [f32; 3],
    pub stretch: f32,
    pub up: [f32; 3],
    pub alpha: f32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub tint: [f32; 3],
    pub quad: u32,
}

pub const VERTS_PER_CUBE: usize = 24;
pub const INDICES_PER_CUBE: usize = 36;

pub const CUBE_INDEX_PATTERN: [u32; INDICES_PER_CUBE] = cube_index_pattern();

const fn cube_index_pattern() -> [u32; INDICES_PER_CUBE] {
    let mut out = [0u32; INDICES_PER_CUBE];
    let mut face = 0;
    while face < 6 {
        let b = face as u32 * 4;
        let at = face * 6;
        out[at] = b;
        out[at + 1] = b + 1;
        out[at + 2] = b + 2;
        out[at + 3] = b;
        out[at + 4] = b + 2;
        out[at + 5] = b + 3;
        face += 1;
    }
    out
}

/// Per-face `right`/`up` basis and directional shade, read by the vertex stage through
/// [`wgsl_faces`]. Faces go +X, -X, +Y, -Y, +Z, -Z. Each face plane is offset outward from the
/// centre by `right.cross(up) * h`, so the six faces form a real cube rather than three squares
/// through the centre.
struct Face {
    right: Vec3,
    up: Vec3,
    shade: f32,
}

const FACES: [Face; 6] = [
    Face {
        right: Vec3::new(0.0, 0.0, -1.0),
        up: Vec3::Y,
        shade: 0.78,
    },
    Face {
        right: Vec3::new(0.0, 0.0, 1.0),
        up: Vec3::Y,
        shade: 0.78,
    },
    Face {
        right: Vec3::X,
        up: Vec3::new(0.0, 0.0, -1.0),
        shade: 1.0,
    },
    Face {
        right: Vec3::X,
        up: Vec3::new(0.0, 0.0, 1.0),
        shade: 0.55,
    },
    Face {
        right: Vec3::X,
        up: Vec3::Y,
        shade: 0.86,
    },
    Face {
        right: Vec3::new(-1.0, 0.0, 0.0),
        up: Vec3::Y,
        shade: 0.86,
    },
];

pub(crate) fn wgsl_faces() -> String {
    let vec3 = |v: Vec3| format!("vec3<f32>({:?}, {:?}, {:?})", v.x, v.y, v.z);
    let list = |f: &dyn Fn(&Face) -> String| FACES.iter().map(f).collect::<Vec<_>>().join(", ");
    format!(
        "var<private> particle_face_right: array<vec3<f32>, 6> = array<vec3<f32>, 6>({});\n\
         var<private> particle_face_up: array<vec3<f32>, 6> = array<vec3<f32>, 6>({});\n\
         var<private> particle_face_shade: array<f32, 6> = array<f32, 6>({});\n",
        list(&|f| vec3(f.right)),
        list(&|f| vec3(f.up)),
        list(&|f| format!("{:?}", f.shade)),
    )
}

#[cfg(test)]
pub fn build_particles(instances: &[ParticleInstance], rows: &mut Vec<ParticleRow>) -> u32 {
    rows.clear();
    rows.extend(
        instances
            .iter()
            .filter(|inst| inst.alpha > 0.0)
            .map(|inst| particle_row(inst, LightEnv::IDENTITY, glam::IVec3::ZERO)),
    );
    rows.len() as u32
}

pub fn build_particles_split(
    block: &[ParticleInstance],
    model: &[ParticleInstance],
    env: LightEnv,
    render_origin: glam::IVec3,
    rows: &mut Vec<ParticleRow>,
) -> (u32, u32) {
    rows.clear();
    let visible = |inst: &&ParticleInstance| inst.alpha > 0.0;
    rows.extend(
        block
            .iter()
            .filter(visible)
            .map(|inst| particle_row(inst, env, render_origin)),
    );
    let block_rows = rows.len() as u32;
    rows.extend(
        model
            .iter()
            .filter(visible)
            .map(|inst| particle_row(inst, env, render_origin)),
    );
    (rows.len() as u32, block_rows)
}

pub struct TransparentParticleCube {
    pos: Vec3,
    color: [f32; 3],
    alpha: f32,
    size: f32,
    stretch: f32,
    dist_sq: f32,
}

#[derive(Copy, Clone)]
struct EmitterSchedule {
    base_gap: f32,
    jitter: f32,
    phase: f32,
    max_rate: f32,
}

/// Builds alpha-blended solid cubes for block-row particle emitters.
/// No stored state. Rows are just seed + time, so move/shrink/fade are recomputed every frame.
///
/// `solids` are droplets the particle sim already placed. Folded in here so splashes and flames
/// sort together.
///
/// Rows and `cam_pos` are relative to `render_origin`. Returns row count.
#[allow(clippy::too_many_arguments)]
pub fn build_transparent_emitter_particles(
    emitters: &[ParticleEmitterInstance],
    solids: &[super::SolidParticleInstance],
    time: f32,
    render_origin: glam::IVec3,
    cam_pos: Vec3,
    env: LightEnv,
    density: f32,
    rows: &mut Vec<ParticleRow>,
    scratch: &mut Vec<TransparentParticleCube>,
) -> u32 {
    rows.clear();
    scratch.clear();
    for s in solids {
        if s.alpha <= 0.001 || s.size <= 0.001 {
            continue;
        }
        let pos = s.pos.relative_to(render_origin);
        scratch.push(TransparentParticleCube {
            pos,
            color: lighting::fold_tint(s.color, DynLight::new(s.skylight, s.blocklight), env),
            alpha: s.alpha,
            size: s.size,
            stretch: s.stretch,
            dist_sq: (cam_pos - pos).length_squared(),
        });
    }
    for inst in emitters {
        append_emitter_particles(inst, time, render_origin, cam_pos, env, density, scratch);
    }
    scratch.sort_by(|a, b| b.dist_sq.total_cmp(&a.dist_sq));
    rows.extend(scratch.iter().map(colored_particle_row));
    rows.len() as u32
}

fn append_emitter_particles(
    inst: &ParticleEmitterInstance,
    time: f32,
    render_origin: glam::IVec3,
    cam_pos: Vec3,
    env: LightEnv,
    density: f32,
    out: &mut Vec<TransparentParticleCube>,
) {
    let e = inst.emitter;
    let max_lifetime = e.lifetime[1].max(e.lifetime[0]);
    let schedule = emitter_schedule(inst.seed, e.rate);
    let active = (((schedule.max_rate * max_lifetime).ceil() as usize + 6) as f32
        * density.clamp(0.0, 1.0))
    .round() as usize;
    let latest = ((time - schedule.phase) / schedule.base_gap).floor() as i64 + 2;
    let light = lighting::fold_self_lit(
        lighting::light_rgb(DynLight::new(inst.skylight, inst.blocklight), env),
        e.self_lit,
    );
    for back in 0..active {
        let seq = latest - back as i64;
        let birth = emitter_birth_time(inst.seed, schedule, seq);
        let age = time - birth;
        if age < 0.0 {
            continue;
        }
        let seed = inst
            .seed
            .wrapping_add((seq as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lifetime = lerp_range(e.lifetime, rand01(seed ^ 0x11));
        if age >= lifetime {
            continue;
        }
        let t = (age / lifetime).clamp(0.0, 1.0);
        let fade = 1.0 - t;
        let size = lerp_range(e.size, rand01(seed ^ 0x22)) * fade.powf(e.shrink_power);
        let alpha = lerp_range(e.alpha, rand01(seed ^ 0x33)) * fade.powf(e.fade_power);
        if size <= 0.001 || alpha <= 0.001 {
            continue;
        }

        let spawn_box = Vec3::from_array(e.spawn_box);
        let jitter = Vec3::new(
            rand_signed(seed ^ 0x44) * spawn_box.x,
            rand_signed(seed ^ 0x55) * spawn_box.y,
            rand_signed(seed ^ 0x66) * spawn_box.z,
        );
        let velocity_jitter = Vec3::from_array(e.velocity_jitter);
        let velocity = Vec3::from_array(e.velocity)
            + Vec3::new(
                rand_signed(seed ^ 0x77) * velocity_jitter.x,
                rand_signed(seed ^ 0x88) * velocity_jitter.y,
                rand_signed(seed ^ 0x99) * velocity_jitter.z,
            );
        let mut pos = inst.origin.relative_to(render_origin) + jitter + velocity * age;
        pos.y -= 0.5 * e.gravity * age * age;
        if pos.y - size * 0.5 <= inst.floor_y - render_origin.y as f32 {
            continue;
        }
        let [spiral_radius, spiral_hz] = e.spiral;
        if spiral_radius > 0.0 {
            let tau = std::f32::consts::TAU;
            let phase = rand01(seed ^ 0xBB) * tau;
            let radius = spiral_radius * lerp(0.6, 1.0, rand01(seed ^ 0xCC));
            let speed = spiral_hz * lerp(0.5, 1.5, rand01(seed ^ 0xDD));
            let angle = phase + speed * tau * age;
            pos += Vec3::new(angle.cos(), 0.0, angle.sin()) * radius;
        }
        let base = match (e.color_ramp, e.color) {
            (Some(ramp), _) => {
                let c = ramp.sample(t);
                let brightness = lerp(0.8, 1.0, rand01(seed ^ 0xEE));
                [c[0] * brightness, c[1] * brightness, c[2] * brightness]
            }
            (None, Some(endpoints)) => {
                let mix = rand01(seed ^ 0xAA);
                [
                    lerp(endpoints[0][0], endpoints[1][0], mix),
                    lerp(endpoints[0][1], endpoints[1][1], mix),
                    lerp(endpoints[0][2], endpoints[1][2], mix),
                ]
            }
            (None, None) => [1.0, 1.0, 1.0],
        };
        let color = lighting::mul3(base, light);
        out.push(TransparentParticleCube {
            pos,
            color,
            alpha,
            size,
            stretch: 1.0,
            dist_sq: (cam_pos - pos).length_squared(),
        });
    }
}

fn emitter_schedule(seed: u64, rate: [f32; 2]) -> EmitterSchedule {
    let min_rate = rate[0];
    let max_rate = rate[1];
    let fastest_gap = 1.0 / max_rate;
    let slowest_gap = 1.0 / min_rate;
    let base_gap = (fastest_gap + slowest_gap) * 0.5;
    let jitter = (slowest_gap - fastest_gap) * 0.25;
    EmitterSchedule {
        base_gap,
        jitter,
        phase: rand01(seed ^ 0xA5A5_517C_D1E5_F00D) * base_gap,
        max_rate,
    }
}

fn emitter_birth_time(seed: u64, schedule: EmitterSchedule, seq: i64) -> f32 {
    let jitter = rand_signed(seed ^ (seq as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    schedule.phase + seq as f32 * schedule.base_gap + jitter * schedule.jitter
}

fn particle_row(inst: &ParticleInstance, env: LightEnv, render_origin: glam::IVec3) -> ParticleRow {
    let [u0, v0] = inst.uv_min;
    let tint = lighting::fold_tint(
        inst.tint,
        DynLight::new(inst.skylight, inst.blocklight),
        env,
    );
    let (right, up, quad) = match inst.quad_axes {
        Some([right, up]) => (right, up, 1),
        None => (Vec3::ZERO, Vec3::ZERO, 0),
    };
    ParticleRow {
        center: inst.pos.relative_to(render_origin).to_array(),
        half: inst.size * 0.5,
        right: right.to_array(),
        stretch: 1.0,
        up: up.to_array(),
        alpha: inst.alpha,
        uv_min: [u0, v0],
        uv_max: [u0 + inst.uv_size[0], v0 + inst.uv_size[1]],
        tint,
        quad,
    }
}

fn colored_particle_row(inst: &TransparentParticleCube) -> ParticleRow {
    ParticleRow {
        center: inst.pos.to_array(),
        half: inst.size * 0.5,
        right: [0.0; 3],
        stretch: inst.stretch,
        up: [0.0; 3],
        alpha: inst.alpha,
        uv_min: [0.0; 2],
        uv_max: [0.0; 2],
        tint: inst.color,
        quad: 0,
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn lerp_range(range: [f32; 2], t: f32) -> f32 {
    lerp(range[0], range[1], t)
}

#[inline]
fn rand01(seed: u64) -> f32 {
    petramond::entity::hash01(seed)
}

#[inline]
fn rand_signed(seed: u64) -> f32 {
    petramond::entity::hash_signed(seed)
}

#[cfg(test)]
mod tests;
