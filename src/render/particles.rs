//! Tiny 3D particle cubes (mining dust + break bursts).
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
//! Geometry is capped to a fixed vertex budget so the dynamic buffer never grows;
//! excess particles in a frame are dropped (transient dust, visually harmless).

use super::lighting;
use super::ParticleInstance;
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

/// Max particle cubes baked per frame. Matches the particle system's pool so a
/// fully saturated pool still draws; the dynamic vbuf is sized to this once.
pub const MAX_PARTICLE_CUBES: usize = crate::entity::PARTICLE_CAPACITY;
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
pub fn build_particles(instances: &[ParticleInstance], verts: &mut Vec<ParticleVertex>) -> u32 {
    verts.clear();
    for inst in instances {
        if verts.len() + VERTS_PER_CUBE > MAX_PARTICLE_VERTICES {
            break;
        }
        if inst.alpha <= 0.0 {
            continue;
        }
        let h = inst.size * 0.5;
        let c = Vec3::from(inst.pos.to_array());
        let [u0, v0] = inst.uv_min;
        let s = inst.uv_size;
        let u1 = u0 + s;
        let v1 = v0 + s;
        let tint = inst.tint;
        let alpha = inst.alpha;
        let light = lighting::sky_light_factor(inst.skylight);
        // UV per face: bl=(u0,v1), br=(u1,v1), tr=(u1,v0), tl=(u0,v0) to match the
        // block pipeline (v grows downward in the atlas). The four corners follow
        // the same CCW order as the uv corners: bl, br, tr, tl.
        for face in &FACES {
            let r = face.right * h;
            let up = face.up * h;
            // Offset the face plane outward along its normal (right x up points
            // out) so each face sits on the cube SURFACE, not through the centre.
            let fc = c + face.right.cross(face.up) * h;
            let shade = face.shade * light;
            let corners = [
                (fc - r - up, [u0, v1]),
                (fc + r - up, [u1, v1]),
                (fc + r + up, [u1, v0]),
                (fc - r + up, [u0, v0]),
            ];
            for (pos, uv) in corners {
                verts.push(ParticleVertex {
                    pos: pos.to_array(),
                    uv,
                    tint,
                    shade,
                    alpha,
                });
            }
        }
    }
    verts.len() as u32
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
        }
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
    fn skylight_scales_particle_face_shades() {
        let mut v = Vec::new();
        let dark = ParticleInstance {
            skylight: 0,
            ..inst(1.0)
        };

        build_particles(std::slice::from_ref(&dark), &mut v);

        assert!(
            v[2 * 4].shade < 1.0,
            "top face should no longer be full bright"
        );
        assert!(
            (v[2 * 4].shade - super::lighting::sky_light_factor(0)).abs() < 1e-6,
            "top face should carry the sampled skylight factor"
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
        let mut v = Vec::with_capacity(MAX_PARTICLE_VERTICES);
        let cap = v.capacity();
        let many = vec![inst(1.0); MAX_PARTICLE_CUBES + 100];
        let n = build_particles(&many, &mut v);
        assert_eq!(
            n as usize, MAX_PARTICLE_VERTICES,
            "capped at the vertex budget"
        );
        assert!(v.capacity() >= cap, "no shrink/regrow churn");
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
