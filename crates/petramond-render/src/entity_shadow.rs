use super::views::EntityShadow;

pub const VERTS_PER_SHADOW: u32 = 4;

pub const QUAD_INDEX_PATTERN: [u32; 6] = [0, 1, 2, 0, 2, 3];

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowVertex {
    pub pos: [f32; 3],
    pub corner: [f32; 2],
    pub strength: f32,
}

pub fn build_entity_shadows(
    shadows: &[EntityShadow],
    render_origin: glam::IVec3,
    verts: &mut Vec<ShadowVertex>,
) -> u32 {
    verts.clear();
    for shadow in shadows {
        let c = shadow.center.relative_to(render_origin);
        let r = shadow.radius;
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

pub fn quad_index_count(quad_count: usize) -> usize {
    quad_count * 6
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    #[test]
    fn shadows_bake_to_ground_hugging_quads_and_cap_out() {
        let mut out = Vec::new();
        let shadow = |x: f32| EntityShadow {
            center: WorldPos::new(f64::from(x), 64.0, 2.0),
            radius: 0.5,
            strength: 0.4,
        };
        assert_eq!(
            build_entity_shadows(&[shadow(1.0)], petramond_math::math::IVec3::ZERO, &mut out),
            4
        );
        assert_eq!(out.len(), 4);
        assert!(
            out.iter().all(|v| (v.pos[1] - 64.0).abs() < f32::EPSILON),
            "every corner sits on the ground plane"
        );
        assert_eq!(out[0].pos[0], 0.5, "centre.x - radius");
        assert_eq!(out[1].pos[0], 1.5, "centre.x + radius");
        assert_eq!(out[2].pos[2], 2.5, "centre.z + radius");

        let many: Vec<_> = (0..2000).map(|i| shadow(i as f32)).collect();
        assert_eq!(
            build_entity_shadows(&many, petramond_math::math::IVec3::ZERO, &mut out),
            2000 * VERTS_PER_SHADOW
        );
    }
}
