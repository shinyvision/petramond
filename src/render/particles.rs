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
//! Geometry is capped to a fixed vertex budget so the dynamic buffer never grows;
//! excess particles in a frame are dropped (transient dust, visually harmless).

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

/// Max particle cubes baked per frame: the simulated pool PLUS equal headroom
/// for the derived ambient volumes (precipitation) that join the same bake at
/// full particle settings. Deliberately on the high end — geometry budgets
/// have bitten before and tiny cubes are cheap; the dynamic vbufs grow on
/// demand up to this, so idle scenes never pay for it.
pub const MAX_PARTICLE_CUBES: usize = crate::entity::PARTICLE_CAPACITY * 2;
/// Vertices in the reusable particle vbuf (24 per cube).
pub const MAX_PARTICLE_VERTICES: usize = MAX_PARTICLE_CUBES * VERTS_PER_CUBE;
/// Indices in the reusable particle ibuf (36 per cube).
pub const MAX_PARTICLE_INDICES: usize = MAX_PARTICLE_CUBES * INDICES_PER_CUBE;

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
/// [`MAX_PARTICLE_VERTICES`]; further particles are dropped. Indices are static
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
        if verts.len() + VERTS_PER_CUBE > MAX_PARTICLE_VERTICES {
            break;
        }
        if inst.alpha <= 0.0 {
            continue;
        }
        push_particle_cube(inst, LightEnv::IDENTITY, verts);
    }
    verts.len() as u32
}

/// Build BLOCK-atlas cubes then MODEL-atlas cubes into ONE vbuf (cleared, capacity
/// reused, total capped at [`MAX_PARTICLE_VERTICES`]). Returns `(total_verts,
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
        if verts.len() + VERTS_PER_CUBE > MAX_PARTICLE_VERTICES {
            break;
        }
        if inst.alpha <= 0.0 {
            continue;
        }
        push_particle_cube(inst, env, verts);
    }
    let block_verts = verts.len() as u32;
    for inst in model {
        if verts.len() + VERTS_PER_CUBE > MAX_PARTICLE_VERTICES {
            break;
        }
        if inst.alpha <= 0.0 {
            continue;
        }
        push_particle_cube(inst, env, verts);
    }
    (verts.len() as u32, block_verts)
}

/// A generated translucent cube particle, sorted by centre distance before vertices
/// are emitted so alpha blending is stable enough for tiny cube puffs.
pub(in crate::render) struct TransparentParticleCube {
    pos: Vec3,
    color: [f32; 3],
    alpha: f32,
    size: f32,
    /// Vertical elongation around the centre (1 = a cube).
    stretch: f32,
    dist_sq: f32,
}

/// Max active translucent particles one emitter row may contribute in a frame.
/// The row's rate/lifetime control the normal count; this clamp prevents a malformed
/// or intentionally huge mod row from consuming the whole fixed vertex buffer
/// (a sliver of [`MAX_PARTICLE_CUBES`]; dense fire columns need more than the
/// original 32).
const MAX_ACTIVE_PER_EMITTER: usize = 48;

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
        if scratch.len() >= MAX_PARTICLE_CUBES {
            break;
        }
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
        if scratch.len() >= MAX_PARTICLE_CUBES {
            break;
        }
    }
    scratch.sort_by(|a, b| b.dist_sq.total_cmp(&a.dist_sq));
    for p in scratch.iter() {
        if verts.len() + VERTS_PER_CUBE > MAX_PARTICLE_VERTICES {
            break;
        }
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
    let active = active.min(MAX_ACTIVE_PER_EMITTER);
    let latest = ((time - schedule.phase) / schedule.base_gap).floor() as i64 + 2;
    let light = if e.fullbright {
        [1.0, 1.0, 1.0]
    } else {
        lighting::light_rgb(DynLight::new(inst.skylight, inst.blocklight), env)
    };
    for back in 0..active {
        if out.len() >= MAX_PARTICLE_CUBES {
            break;
        }
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
    let s = inst.uv_size;
    let u1 = u0 + s;
    let v1 = v0 + s;
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
    crate::entity::hash01(seed)
}

#[inline]
fn rand_signed(seed: u64) -> f32 {
    crate::entity::hash_signed(seed)
}

/// The static index buffer for [`MAX_PARTICLE_CUBES`] cubes (six faces, two
/// triangles each, CCW: 0,1,2, 0,2,3 per face). Built once and uploaded at
/// startup; draws use the slice matching the live cube count.
pub fn particle_indices() -> Vec<u32> {
    let mut idx = Vec::with_capacity(MAX_PARTICLE_INDICES);
    for cube in 0..MAX_PARTICLE_CUBES as u32 {
        let cube_base = cube * VERTS_PER_CUBE as u32;
        for face in 0..6u32 {
            let b = cube_base + face * 4;
            idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn inst(alpha: f32) -> ParticleInstance {
        ParticleInstance {
            pos: Vec3::new(1.0, 2.0, 3.0),
            uv_min: [0.1, 0.2],
            uv_size: 0.05,
            tint: [1.0, 1.0, 1.0],
            alpha,
            size: 0.1,
            skylight: lighting::FULL_SKYLIGHT,
            blocklight: 0,
        }
    }

    fn emitter_inst() -> ParticleEmitterInstance {
        ParticleEmitterInstance {
            origin: Vec3::new(1.0, 2.0, 3.0),
            emitter: crate::block::ParticleEmitter {
                anchor: crate::block::ParticleEmitterAnchor::BlockTop,
                origin: [0.5, 1.0, 0.5],
                offset: [0.0, 0.0, 0.0],
                rate: [1.0, 1.0],
                lifetime: [1.0, 1.0],
                size: [0.2, 0.2],
                spawn_box: [0.0, 0.0, 0.0],
                velocity: [0.0, 1.0, 0.0],
                velocity_jitter: [0.0, 0.0, 0.0],
                color: Some([[1.0, 0.5, 0.0], [1.0, 1.0, 0.0]]),
                color_ramp: None,
                alpha: [0.8, 0.8],
                fade_power: 2.0,
                shrink_power: 1.0,
                fullbright: true,
                spiral: [0.0, 0.0],
            },
            seed: 0x1234_5678_9ABC_DEF0,
            skylight: 0,
            blocklight: 0,
        }
    }

    fn one_live_emitter_time(inst: &ParticleEmitterInstance, age: f32) -> f32 {
        let schedule = emitter_schedule(inst.seed, inst.emitter.rate);
        // Sequence 10 keeps the test time positive for every phase in [0, 1).
        emitter_birth_time(inst.seed, schedule, 10) + age
    }

    fn vertex_center(v: &[ParticleVertex]) -> Vec3 {
        let sum = v.iter().fold(Vec3::ZERO, |acc, p| acc + Vec3::from(p.pos));
        sum / v.len() as f32
    }

    fn x_extent(v: &[ParticleVertex]) -> f32 {
        let min_x = v.iter().map(|p| p.pos[0]).fold(f32::INFINITY, f32::min);
        let max_x = v.iter().map(|p| p.pos[0]).fold(f32::NEG_INFINITY, f32::max);
        max_x - min_x
    }

    fn max_alpha(v: &[ParticleVertex]) -> f32 {
        v.iter().map(|p| p.alpha).fold(0.0, f32::max)
    }

    #[test]
    fn each_visible_particle_is_one_cube() {
        let mut v = Vec::new();
        let n = build_particles(&[inst(1.0), inst(0.5)], &mut v);
        assert_eq!(
            n as usize,
            2 * VERTS_PER_CUBE,
            "two particles = two cubes = 48 verts"
        );
        assert_eq!(v.len(), 2 * VERTS_PER_CUBE);
        // Alpha is carried per vertex.
        assert_eq!(v[0].alpha, 1.0);
        assert_eq!(v[VERTS_PER_CUBE].alpha, 0.5);
    }

    #[test]
    fn tint_is_carried_to_every_vertex() {
        let green = ParticleInstance {
            tint: [0.5, 0.72, 0.38],
            ..inst(1.0)
        };
        let mut v = Vec::new();
        build_particles(std::slice::from_ref(&green), &mut v);
        assert_eq!(v.len(), VERTS_PER_CUBE);
        for vert in &v {
            assert_eq!(
                vert.tint,
                [0.5, 0.72, 0.38],
                "every cube vertex carries the tint"
            );
        }
    }

    #[test]
    fn faces_carry_distinct_directional_shades() {
        let mut v = Vec::new();
        build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
        // Top face (index 2) is brightest, bottom (index 3) darkest.
        let top = v[2 * 4].shade;
        let bottom = v[3 * 4].shade;
        let side = v[0].shade;
        assert!(
            top > side && side > bottom,
            "top > side > bottom shading reads 3D"
        );
        assert_eq!(top, 1.0);
    }

    #[test]
    fn sampled_light_folds_into_the_particle_tint() {
        // The two-channel RGB light rides the vertex TINT (shade keeps only the
        // directional term), so a dark sample dims the tint, not the shade.
        let mut v = Vec::new();
        let dark = ParticleInstance {
            skylight: 0,
            ..inst(1.0)
        };

        build_particles(std::slice::from_ref(&dark), &mut v);

        assert_eq!(v[2 * 4].shade, 1.0, "shade stays directional-only");
        let expect = lighting::light_rgb(DynLight { sky: 0, block: 0 }, LightEnv::IDENTITY);
        assert_eq!(v[2 * 4].tint, expect, "unlit sample dims the tint");
        assert!(expect[0] < 1.0);
    }

    #[test]
    fn block_emitter_particles_rise_shrink_and_fade() {
        let inst = emitter_inst();
        let mut young = Vec::new();
        let mut old = Vec::new();
        let mut scratch = Vec::new();

        let young_n = build_transparent_emitter_particles(
            std::slice::from_ref(&inst),
            &[],
            one_live_emitter_time(&inst, 0.25),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut young,
            &mut scratch,
        );
        let old_n = build_transparent_emitter_particles(
            std::slice::from_ref(&inst),
            &[],
            one_live_emitter_time(&inst, 0.75),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut old,
            &mut scratch,
        );

        assert_eq!(young_n as usize, VERTS_PER_CUBE);
        assert_eq!(old_n as usize, VERTS_PER_CUBE);
        assert!(
            vertex_center(&old).y > vertex_center(&young).y,
            "emitter particles move upward over their lifetime"
        );
        assert!(
            x_extent(&old) < x_extent(&young),
            "emitter particles shrink as they age"
        );
        assert!(
            max_alpha(&old) < max_alpha(&young),
            "emitter particles fade as they age"
        );
    }

    #[test]
    fn spiral_emitter_particles_orbit_the_vertical_axis_as_they_age() {
        let mut inst = emitter_inst();
        inst.emitter.spiral = [0.5, 1.0]; // one revolution per second at 0.5 blocks
        let mut early = Vec::new();
        let mut late = Vec::new();
        let mut scratch = Vec::new();

        // A quarter revolution apart: the particle's horizontal offset from the
        // emitter axis must keep its radius but rotate to a different angle.
        build_transparent_emitter_particles(
            std::slice::from_ref(&inst),
            &[],
            one_live_emitter_time(&inst, 0.25),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut early,
            &mut scratch,
        );
        build_transparent_emitter_particles(
            std::slice::from_ref(&inst),
            &[],
            one_live_emitter_time(&inst, 0.5),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut late,
            &mut scratch,
        );

        let axis = inst.origin;
        let horiz = |v: &[ParticleVertex]| {
            let c = vertex_center(v);
            Vec3::new(c.x - axis.x, 0.0, c.z - axis.z)
        };
        let (a, b) = (horiz(&early), horiz(&late));
        // Orbit radius is per-particle (60-100% of the row's 0.5) but stable
        // over one particle's life: both samples see the same particle.
        assert!(
            (a.length() - b.length()).abs() < 1e-4,
            "one particle keeps its orbit radius: {} vs {}",
            a.length(),
            b.length()
        );
        assert!(
            (0.3 - 1e-4..=0.5 + 1e-4).contains(&a.length()),
            "orbit radius stays within the row's spiral radius: {}",
            a.length()
        );
        assert!(
            a.angle_between(b) > 0.3,
            "a quarter nominal revolution rotates the offset (angle {})",
            a.angle_between(b)
        );
    }

    #[test]
    fn ramp_emitter_particles_cool_through_the_ramp_as_they_age() {
        let mut inst = emitter_inst();
        let ramp: crate::block::ColorRamp =
            serde_json::from_str(r#"[[1.0, 1.0, 0.9], [1.0, 0.5, 0.1], [0.1, 0.1, 0.1]]"#)
                .expect("3-stop ramp parses");
        inst.emitter.color = None;
        inst.emitter.color_ramp = Some(ramp);
        let mut young = Vec::new();
        let mut old = Vec::new();
        let mut scratch = Vec::new();

        build_transparent_emitter_particles(
            std::slice::from_ref(&inst),
            &[],
            one_live_emitter_time(&inst, 0.1),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut young,
            &mut scratch,
        );
        build_transparent_emitter_particles(
            std::slice::from_ref(&inst),
            &[],
            one_live_emitter_time(&inst, 0.9),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut old,
            &mut scratch,
        );

        let luma = |v: &[ParticleVertex]| {
            let t = v[0].tint;
            t[0] + t[1] + t[2]
        };
        assert!(
            luma(&young) > 2.0,
            "a young particle sits near the hot end of the ramp: {}",
            luma(&young)
        );
        assert!(
            luma(&old) < 1.0,
            "an old particle has cooled toward the dark end: {}",
            luma(&old)
        );
    }

    #[test]
    fn lower_fade_power_keeps_late_life_particles_more_visible() {
        let quick = emitter_inst();
        let mut lingering = emitter_inst();
        lingering.emitter.fade_power = 1.0;
        let mut a = Vec::new();
        let mut b = Vec::new();
        let mut scratch = Vec::new();

        build_transparent_emitter_particles(
            std::slice::from_ref(&quick),
            &[],
            one_live_emitter_time(&quick, 0.75),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut a,
            &mut scratch,
        );
        build_transparent_emitter_particles(
            std::slice::from_ref(&lingering),
            &[],
            one_live_emitter_time(&lingering, 0.75),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut b,
            &mut scratch,
        );

        assert!(
            max_alpha(&b) > max_alpha(&a),
            "fade_power 1 lingers longer than the default quadratic: {} vs {}",
            max_alpha(&b),
            max_alpha(&a)
        );
    }

    #[test]
    fn lower_shrink_power_keeps_late_life_particles_larger() {
        let linear = emitter_inst();
        let mut chunky = emitter_inst();
        chunky.emitter.shrink_power = 0.4;
        let mut a = Vec::new();
        let mut b = Vec::new();
        let mut scratch = Vec::new();

        build_transparent_emitter_particles(
            std::slice::from_ref(&linear),
            &[],
            one_live_emitter_time(&linear, 0.75),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut a,
            &mut scratch,
        );
        build_transparent_emitter_particles(
            std::slice::from_ref(&chunky),
            &[],
            one_live_emitter_time(&chunky, 0.75),
            Vec3::ZERO,
            LightEnv::IDENTITY,
            1.0,
            &mut b,
            &mut scratch,
        );

        assert!(
            x_extent(&b) > x_extent(&a),
            "shrink_power 0.4 keeps an old cube chunkier than linear shrink: {} vs {}",
            x_extent(&b),
            x_extent(&a)
        );
    }

    #[test]
    fn block_emitter_rate_range_jitters_spawn_intervals() {
        let seed = 0xCAFE_BABE_F00D_1234;
        let schedule = emitter_schedule(seed, [1.0, 2.0]);
        let mut saw_variation = false;
        let mut prev = emitter_birth_time(seed, schedule, 0);

        for seq in 1..64 {
            let birth = emitter_birth_time(seed, schedule, seq);
            let gap = birth - prev;
            assert!(
                (0.5..=1.0).contains(&gap),
                "rate range 1.0-2.0 should produce gaps in 0.5-1.0 seconds, got {gap}"
            );
            saw_variation |= (gap - schedule.base_gap).abs() > 0.01;
            prev = birth;
        }

        assert!(
            saw_variation,
            "range emitters should not spawn on a fixed cadence"
        );
    }

    #[test]
    fn fully_faded_particles_are_skipped() {
        let mut v = Vec::new();
        let n = build_particles(&[inst(0.0), inst(1.0)], &mut v);
        assert_eq!(
            n as usize, VERTS_PER_CUBE,
            "the alpha=0 particle is dropped"
        );
    }

    #[test]
    fn cube_is_centred_on_pos() {
        let mut v = Vec::new();
        build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
        let cx: f32 = v.iter().map(|p| p.pos[0]).sum::<f32>() / v.len() as f32;
        let cy: f32 = v.iter().map(|p| p.pos[1]).sum::<f32>() / v.len() as f32;
        let cz: f32 = v.iter().map(|p| p.pos[2]).sum::<f32>() / v.len() as f32;
        assert!((cx - 1.0).abs() < 1e-5 && (cy - 2.0).abs() < 1e-5 && (cz - 3.0).abs() < 1e-5);
    }

    #[test]
    fn cube_extent_matches_size() {
        let mut v = Vec::new();
        build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
        let min_x = v.iter().map(|p| p.pos[0]).fold(f32::INFINITY, f32::min);
        let max_x = v.iter().map(|p| p.pos[0]).fold(f32::NEG_INFINITY, f32::max);
        // Side length == size (0.1), so extent on each axis is the full size.
        assert!(
            (max_x - min_x - 0.1).abs() < 1e-5,
            "cube spans `size` on each axis"
        );
    }

    #[test]
    fn faces_are_offset_to_the_cube_surface_not_the_centre() {
        // Regression for the "star/+" bug: every face used to pass through the
        // cube centre (corners = c +/- r +/- up). A real cube has each face
        // offset outward by `normal*h`, giving 8 distinct corner positions.
        let mut v = Vec::new();
        build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
        let c = Vec3::new(1.0, 2.0, 3.0);
        let h = 0.1 * 0.5; // size 0.1
                           // +X face is FACES[0]; its 4 verts must all sit on the +X plane
                           // (x=c.x+h), NOT through the centre (x=c.x). -X face (FACES[1]) sits at
                           // x=c.x-h.
        for i in 0..4 {
            assert!(
                (v[i].pos[0] - (c.x + h)).abs() < 1e-6,
                "+X face on the +X surface"
            );
            assert!(
                (v[4 + i].pos[0] - (c.x - h)).abs() < 1e-6,
                "-X face on the -X surface"
            );
        }
        // +Y / -Y faces (FACES[2], [3]) on the top/bottom planes.
        for i in 0..4 {
            assert!(
                (v[8 + i].pos[1] - (c.y + h)).abs() < 1e-6,
                "+Y face on the top surface"
            );
            assert!(
                (v[12 + i].pos[1] - (c.y - h)).abs() < 1e-6,
                "-Y face on the bottom surface"
            );
        }
        // +Z / -Z faces (FACES[4], [5]) on the front/back planes.
        for i in 0..4 {
            assert!(
                (v[16 + i].pos[2] - (c.z + h)).abs() < 1e-6,
                "+Z face on the +Z surface"
            );
            assert!(
                (v[20 + i].pos[2] - (c.z - h)).abs() < 1e-6,
                "-Z face on the -Z surface"
            );
        }
        // A real cube has exactly 8 distinct corner positions (the 24 verts are
        // the 8 corners shared 3 ways). The buggy star had only 6 (centre-crossed
        // squares share the 4 mid-edge points differently); assert 8 here.
        let mut corners: Vec<[i32; 3]> = v
            .iter()
            .map(|p| {
                [
                    (p.pos[0] * 1e4) as i32,
                    (p.pos[1] * 1e4) as i32,
                    (p.pos[2] * 1e4) as i32,
                ]
            })
            .collect();
        corners.sort_unstable();
        corners.dedup();
        assert_eq!(
            corners.len(),
            8,
            "a real cube has 8 distinct corner positions"
        );
    }

    #[test]
    fn caps_at_capacity_and_reuses_buffer() {
        let mut v = Vec::new();
        let many = vec![inst(1.0); MAX_PARTICLE_CUBES + 100];
        let n = build_particles(&many, &mut v);
        assert_eq!(
            n as usize, MAX_PARTICLE_VERTICES,
            "capped at the vertex budget"
        );
        let cap = v.capacity();
        // Same input -> identical (capped) vert count, so the cleared+refilled
        // buffer keeps its capacity: rebuilding to the same size never reallocs.
        let n = build_particles(&many, &mut v);
        assert_eq!(n as usize, MAX_PARTICLE_VERTICES);
        assert_eq!(v.capacity(), cap, "vert buffer reused");
    }

    #[test]
    fn index_buffer_is_thirtysix_per_cube() {
        let idx = particle_indices();
        assert_eq!(idx.len(), MAX_PARTICLE_INDICES);
        // First face of first cube: 0,1,2, 0,2,3.
        assert_eq!(&idx[..6], &[0, 1, 2, 0, 2, 3]);
        // Second face starts at vertex 4.
        assert_eq!(&idx[6..12], &[4, 5, 6, 4, 6, 7]);
        // Second cube starts at vertex 24.
        let c2 = INDICES_PER_CUBE;
        assert_eq!(&idx[c2..c2 + 6], &[24, 25, 26, 24, 26, 27]);
    }
}
