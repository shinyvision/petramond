//! Tiny 3D particle cubes.
//!
//! Each [`ParticleInstance`] (world pos + **absolute** atlas uv patch + tint +
//! alpha + size) is expanded into a small textured CUBE each frame (NOT a
//! camera-facing billboard) so dust is visible from any angle, including from
//! directly above. Six faces, each textured with the particle's sub-patch of the
//! block atlas (the absolute `uv_min` + `uv_size`), multiplied by the particle
//! tint and a per-face directional shade so the cube reads as a solid 3D nugget.
//!
//! Geometry is built CPU-side into a reusable dynamic vbuf with a compact
//! per-vertex format ([`ParticleVertex`]: pos + uv + tint + shade + alpha =
//! 40 bytes). The dedicated `particles.wgsl` pipeline transforms by `view_proj`,
//! samples the atlas, applies `shade * tint`, and uses an alpha **cutout**
//! (discard a<0.5) so the cubes are depth-TESTED *and* depth-WRITTEN — correctly
//! occluded by terrain, visible from above, and mutually self-sorting. Particles
//! fade near end-of-life by SHRINKING the cube (alpha is folded into the cutout).
//!
//! Block-row emitters reuse the same vertex format but bake solid-colour cubes for
//! a separate alpha-blended pipeline. Those cubes are presentation-only, sorted
//! far-to-near before vertex emission, and back-face culled by the render pipeline
//! so tiny transparent flames do not reveal all six faces at once.
//!
//! Geometry is unbounded: every live particle bakes, and the dynamic buffer
//! behind it grows to fit (`DynamicVertexDraw`).

use super::lighting::{self, DynLight, LightEnv};
use super::{ParticleEmitterInstance, ParticleInstance};
use glam::Vec3;

/// Compact particle vertex: world position + absolute atlas uv + RGB tint +
/// per-face shade + alpha. 40 bytes, matching the `particles.wgsl` `VsIn` and the
/// pipeline's vertex attributes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub tint: [f32; 3],
    /// Per-face directional shade (0..1) baked CPU-side so the cube reads 3D.
    pub shade: f32,
    pub alpha: f32,
}

/// Vertices per particle cube (6 faces * 4 verts, indexed; no shared verts so
/// each face carries its own uv + shade).
pub const VERTS_PER_CUBE: usize = 24;
/// Indices per particle cube (6 faces * 2 triangles * 3).
pub const INDICES_PER_CUBE: usize = 36;

/// The relative index pattern of one cube: six faces, two CCW triangles each
/// (0,1,2, 0,2,3 per face), over [`VERTS_PER_CUBE`] consecutive vertices.
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

/// Per-face data: the in-plane basis (`right`/`up`) and the directional shade.
/// Faces are ordered +X, -X, +Y, -Y, +Z, -Z. The face plane is offset outward
/// from the cube centre by `right.cross(up) * h` (the cross points outward), so
/// the four corners are `centre + normal*h +/- right*h +/- up*h` — i.e. the six
/// faces form a real cube rather than three squares through the centre.
struct Face {
    right: Vec3,
    up: Vec3,
    shade: f32,
}

/// The six cube faces with a fixed directional shade so the cube reads 3D from
/// any angle: top brightest, sides mid, bottom darkest (matches the block
/// pipeline's ambient face shading convention). `right`/`up` are wound CCW when
/// viewed from outside so a single winding is visible without backface tricks
/// (the pipeline disables culling regardless).
const FACES: [Face; 6] = [
    // +X (east)
    Face {
        right: Vec3::new(0.0, 0.0, -1.0),
        up: Vec3::Y,
        shade: 0.78,
    },
    // -X (west)
    Face {
        right: Vec3::new(0.0, 0.0, 1.0),
        up: Vec3::Y,
        shade: 0.78,
    },
    // +Y (top)
    Face {
        right: Vec3::X,
        up: Vec3::new(0.0, 0.0, -1.0),
        shade: 1.0,
    },
    // -Y (bottom)
    Face {
        right: Vec3::X,
        up: Vec3::new(0.0, 0.0, 1.0),
        shade: 0.55,
    },
    // +Z (south)
    Face {
        right: Vec3::X,
        up: Vec3::Y,
        shade: 0.86,
    },
    // -Z (north)
    Face {
        right: Vec3::new(-1.0, 0.0, 0.0),
        up: Vec3::Y,
        shade: 0.86,
    },
];

/// Build tiny 3D cubes for `instances` into `verts` (cleared, capacity reused).
/// Returns the **vertex** count written (24 per cube). Caps at
/// every instance. Indices are the static cube pattern
/// (see [`particle_indices`]) so only the vbuf is rewritten each frame.
///
/// Each cube is centred at `inst.pos` with side `inst.size`; the renderer shrinks
/// the size near end-of-life so a fading cube also shrinks. Every face samples
/// the particle's absolute atlas patch (`uv_min` + `uv_size`) tinted by
/// `inst.tint` and shaded per-face.
/// Block-atlas-only cube builder, kept as the focused unit-test entry for the per-cube
/// geometry (faces, shades, centring, caps). The renderer uses [`build_particles_split`].
#[cfg(test)]
pub fn build_particles(instances: &[ParticleInstance], verts: &mut Vec<ParticleVertex>) -> u32 {
    verts.clear();
    for inst in instances {
        if inst.alpha <= 0.0 {
            continue;
        }
        push_particle_cube(inst, LightEnv::IDENTITY, verts);
    }
    verts.len() as u32
}

/// Build BLOCK-atlas cubes then MODEL-atlas cubes into ONE vbuf (cleared, capacity
/// reused). Returns `(total_verts,
/// block_verts)` — the renderer draws `[0..block_verts)` with the block atlas bound and
/// `[block_verts..total)` with the model atlas bound, so bbmodel-block flecks sample
/// their own texture in the same pass. Block cubes come first so the split is a single
/// contiguous index boundary.
pub fn build_particles_split(
    block: &[ParticleInstance],
    model: &[ParticleInstance],
    env: LightEnv,
    verts: &mut Vec<ParticleVertex>,
) -> (u32, u32) {
    verts.clear();
    for inst in block {
        if inst.alpha <= 0.0 {
            continue;
        }
        push_particle_cube(inst, env, verts);
    }
    let block_verts = verts.len() as u32;
    for inst in model {
        if inst.alpha <= 0.0 {
            continue;
        }
        push_particle_cube(inst, env, verts);
    }
    (verts.len() as u32, block_verts)
}

/// A generated translucent cube particle, sorted by centre distance before vertices
/// are emitted so alpha blending is stable enough for tiny cube puffs.
pub struct TransparentParticleCube {
    pos: Vec3,
    color: [f32; 3],
    alpha: f32,
    size: f32,
    /// Vertical elongation around the centre (1 = a cube).
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

/// Build alpha-blended solid-color cubes for block-row particle emitters. The
/// generated particle rows are deterministic functions of `(emitter seed, time)`,
/// so no persistent particle state is needed: a particle moves up, shrinks, fades,
/// and disappears entirely on the render side.
///
/// `solids` are the SIMULATED solid-color particles (emitter-burst droplets,
/// already positioned by the particle system's physics): they join the same
/// sorted alpha-blended draw so splashes and flames composite correctly.
pub fn build_transparent_emitter_particles(
    emitters: &[ParticleEmitterInstance],
    solids: &[super::SolidParticleInstance],
    time: f32,
    cam_pos: Vec3,
    env: LightEnv,
    density: f32,
    verts: &mut Vec<ParticleVertex>,
    scratch: &mut Vec<TransparentParticleCube>,
) -> u32 {
    verts.clear();
    scratch.clear();
    for s in solids {
        if s.alpha <= 0.001 || s.size <= 0.001 {
            continue;
        }
        scratch.push(TransparentParticleCube {
            pos: s.pos,
            color: lighting::fold_tint(s.color, DynLight::new(s.skylight, s.blocklight), env),
            alpha: s.alpha,
            size: s.size,
            stretch: s.stretch,
            dist_sq: (cam_pos - s.pos).length_squared(),
        });
    }
    for inst in emitters {
        append_emitter_particles(inst, time, cam_pos, env, density, scratch);
    }
    scratch.sort_by(|a, b| b.dist_sq.total_cmp(&a.dist_sq));
    for p in scratch.iter() {
        push_colored_particle_cube(p, verts);
    }
    verts.len() as u32
}

fn append_emitter_particles(
    inst: &ParticleEmitterInstance,
    time: f32,
    cam_pos: Vec3,
    env: LightEnv,
    density: f32,
    out: &mut Vec<TransparentParticleCube>,
) {
    let e = inst.emitter;
    let max_lifetime = e.lifetime[1].max(e.lifetime[0]);
    let schedule = emitter_schedule(inst.seed, e.rate);
    // The particles graphics option thins each emitter's active window
    // (reduced = half density); zero is culled before this is reached.
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
        // The row's exponents shape the curves: fade_power 2 / shrink_power 1
        // are the classic quick fade + linear shrink; lower keeps late-life
        // (ember/smoke) cubes visible and chunky.
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
        let mut pos = inst.origin + jitter + velocity * age;
        // Spiral: each particle orbits the emitter's vertical axis while it
        // rises. Phase, orbit radius, AND angular speed are all per-particle
        // (seed-derived): a shared speed reads as a rigid rotating helix, while
        // individual orbits twirl unpredictably, like flame licks. The row's
        // values are the outer radius / nominal speed.
        let [spiral_radius, spiral_hz] = e.spiral;
        if spiral_radius > 0.0 {
            let tau = std::f32::consts::TAU;
            let phase = rand01(seed ^ 0xBB) * tau;
            let radius = spiral_radius * lerp(0.6, 1.0, rand01(seed ^ 0xCC));
            let speed = spiral_hz * lerp(0.5, 1.5, rand01(seed ^ 0xDD));
            let angle = phase + speed * tau * age;
            pos += Vec3::new(angle.cos(), 0.0, angle.sin()) * radius;
        }
        // Color: a ramp row COOLS over the particle's life (age maps to height
        // in a rising column, so the base burns white-hot and the top chars),
        // with a small per-particle brightness jitter for texture; an endpoint
        // row keeps its classic random birth mix.
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
            // The loader guarantees one of the two; render defensively.
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

/// Append one particle's textured cube (24 verts) to `verts`. Every face samples the
/// particle's absolute atlas patch (`uv_min` + `uv_size`) tinted by `inst.tint` and
/// shaded per-face. The caller does the capacity + alpha gating.
fn push_particle_cube(inst: &ParticleInstance, env: LightEnv, verts: &mut Vec<ParticleVertex>) {
    let [u0, v0] = inst.uv_min;
    let u1 = u0 + inst.uv_size[0];
    let v1 = v0 + inst.uv_size[1];
    // Two-channel RGB light folds into the tint (shade keeps the directional
    // term), so a fleck drifting through torch light stays lit at night.
    let tint = lighting::fold_tint(
        inst.tint,
        DynLight::new(inst.skylight, inst.blocklight),
        env,
    );
    // UV per face: bl=(u0,v1), br=(u1,v1), tr=(u1,v0), tl=(u0,v0) to match the
    // block pipeline (v grows downward in the atlas). The four corners follow
    // the same CCW order as the uv corners: bl, br, tr, tl.
    let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
    push_cube_faces(
        Vec3::from(inst.pos.to_array()),
        inst.size,
        corner_uv,
        tint,
        inst.alpha,
        verts,
    );
}

fn push_colored_particle_cube(inst: &TransparentParticleCube, verts: &mut Vec<ParticleVertex>) {
    push_stretched_cube_faces(
        inst.pos,
        inst.size,
        inst.stretch,
        [[0.0, 0.0]; 4],
        inst.color,
        inst.alpha,
        verts,
    );
}

/// Emit the six shaded faces (24 verts) of one particle cube of side `size`
/// centred at `c`, with per-corner UVs (bl, br, tr, tl order) shared by every
/// face. The textured and solid-colour builders differ only in what they feed in.
fn push_cube_faces(
    c: Vec3,
    size: f32,
    corner_uv: [[f32; 2]; 4],
    tint: [f32; 3],
    alpha: f32,
    verts: &mut Vec<ParticleVertex>,
) {
    push_stretched_cube_faces(c, size, 1.0, corner_uv, tint, alpha, verts);
}

/// [`push_cube_faces`] with a vertical elongation: each vertex's y is scaled
/// by `stretch` around the centre, turning the cube into a tall box (rain
/// streaks) while faces stay planar.
#[allow(clippy::too_many_arguments)]
fn push_stretched_cube_faces(
    c: Vec3,
    size: f32,
    stretch: f32,
    corner_uv: [[f32; 2]; 4],
    tint: [f32; 3],
    alpha: f32,
    verts: &mut Vec<ParticleVertex>,
) {
    let h = size * 0.5;
    for face in &FACES {
        let r = face.right * h;
        let up = face.up * h;
        // Offset the face plane outward along its normal (right x up points
        // out) so each face sits on the cube SURFACE, not through the centre.
        let fc = c + face.right.cross(face.up) * h;
        let corners = [
            (fc - r - up, corner_uv[0]),
            (fc + r - up, corner_uv[1]),
            (fc + r + up, corner_uv[2]),
            (fc - r + up, corner_uv[3]),
        ];
        for (mut pos, uv) in corners {
            pos.y = c.y + (pos.y - c.y) * stretch;
            verts.push(ParticleVertex {
                pos: pos.to_array(),
                uv,
                tint,
                shade: face.shade,
                alpha,
            });
        }
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
