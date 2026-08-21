//! Entity blob shadows: soft radial darkening decals stamped on the ground
//! under mobs, dropped items, and player bodies.
//!
//! The decal rows ([`EntityShadow`]) are resolved by the presentation gather —
//! only it has the world to find the ground below an entity and each body's
//! footprint to scale the radius. The renderer's part is purely geometric:
//! one horizontal quad per row, drawn by a MULTIPLY-blended, depth-read-only
//! pass right after the contact-shadow pass (same safety contract — see that
//! pass for why it draws before the sky). The radial falloff lives in the
//! fragment shader (`entity_shadow.wgsl`), so a quad costs 4 vertices and the
//! soft edge is free.

use super::views::EntityShadow;

/// Most shadow quads one frame may draw. Entities visible in any ordinary view
/// number in the dozens; this cap is generous headroom, and the bake simply
/// stops at it (nearest shadows aren't specially picked — overflow is a scene
/// far outside anything the culling lets through).
pub const MAX_ENTITY_SHADOWS: usize = 512;

/// Vertices per shadow quad (a non-shared static index buffer pairs them into
/// two triangles).
pub const VERTS_PER_SHADOW: u32 = 4;

/// One shadow-quad corner: world-space position, the corner's signed unit
/// offset from the centre (the fragment shader derives the radial falloff
/// from its length), and the peak darkening.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowVertex {
    pub pos: [f32; 3],
    pub corner: [f32; 2],
    pub strength: f32,
}

/// Build one horizontal quad per shadow row into `verts` (cleared + refilled,
/// capacity reused), stopping at [`MAX_ENTITY_SHADOWS`]. Returns the vertex
/// count emitted; the pass draws `(count / VERTS_PER_SHADOW) * 6` indices from
/// the static quad ibuf.
pub fn build_entity_shadows(shadows: &[EntityShadow], verts: &mut Vec<ShadowVertex>) -> u32 {
    verts.clear();
    for shadow in shadows.iter().take(MAX_ENTITY_SHADOWS) {
        let c = shadow.center;
        let r = shadow.radius;
        // Corner order matches the static quad indices (0,1,2 / 0,2,3):
        // (-,-) (+,-) (+,+) (-,+).
        let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for &(cx, cz) in &corners {
            verts.push(ShadowVertex {
                pos: [c.x + cx * r, c.y, c.z + cz * r],
                corner: [cx, cz],
                strength: shadow.strength,
            });
        }
    }
    verts.len() as u32
}

/// The static per-quad index pattern, repeated per quad slot (indices are
/// relative to each quad's first vertex; the draw uses no base vertex because
/// every quad's indices are absolute into the shared vbuf — hence the multiply
/// here rather than a `base_vertex`).
pub fn quad_index_count(quad_count: usize) -> usize {
    quad_count * 6
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn shadows_bake_to_ground_hugging_quads_and_cap_out() {
        let mut out = Vec::new();
        let shadow = |x: f32| EntityShadow {
            center: Vec3::new(x, 64.0, 2.0),
            radius: 0.5,
            strength: 0.4,
        };
        // One row → exactly 4 vertices forming the quad around the centre at
        // ground height y=64.
        assert_eq!(build_entity_shadows(&[shadow(1.0)], &mut out), 4);
        assert_eq!(out.len(), 4);
        assert!(
            out.iter().all(|v| (v.pos[1] - 64.0).abs() < f32::EPSILON),
            "every corner sits on the ground plane"
        );
        assert_eq!(out[0].pos[0], 0.5, "centre.x - radius");
        assert_eq!(out[1].pos[0], 1.5, "centre.x + radius");
        assert_eq!(out[2].pos[2], 2.5, "centre.z + radius");

        // More rows than the cap → the bake stops at the cap instead of growing
        // past the fixed GPU buffers.
        let many: Vec<_> = (0..MAX_ENTITY_SHADOWS + 16)
            .map(|i| shadow(i as f32))
            .collect();
        assert_eq!(
            build_entity_shadows(&many, &mut out),
            (MAX_ENTITY_SHADOWS as u32) * VERTS_PER_SHADOW
        );
    }
}
