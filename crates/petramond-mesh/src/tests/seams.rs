use super::*;

#[test]
fn cross_section_seam_culls_faces_and_samples_neighbour_ao_and_light() {
    let mut lower = Section::new(0, 0, 0);
    lower.set_block(8, 15, 8, Block::Stone);
    lower.set_block(9, 15, 8, Block::Stone);
    let mut upper = Section::new(0, 1, 0);
    upper.set_block(9, 0, 8, Block::Stone);

    let block_at = |wx: i32, wy: i32, wz: i32| -> u16 {
        if !(0..SECTION_SIZE as i32).contains(&wx) || !(0..SECTION_SIZE as i32).contains(&wz) {
            return Block::Air.id();
        }
        match wy {
            0..=15 => lower.block_raw(wx as usize, wy as usize, wz as usize),
            16..=31 => upper.block_raw(wx as usize, (wy - 16) as usize, wz as usize),
            _ => Block::Air.id(),
        }
    };
    let light_at = |_: i32, wy: i32, _: i32| -> u8 {
        if wy >= 16 {
            SKY_FULL
        } else {
            0
        }
    };

    let lower_mesh = mesh_in_scene(&lower, SectionPos::new(0, 0, 0), block_at, light_at);
    let upper_mesh = mesh_in_scene(&upper, SectionPos::new(0, 1, 0), block_at, light_at);

    for (name, mesh) in [("lower", &lower_mesh), ("upper", &upper_mesh)] {
        for quad in mesh.opaque.chunks(4) {
            let on_seam_cell = quad.iter().all(|v| {
                (v.pos[1] - 16.0).abs() < 1e-3
                    && v.pos[0] >= 9.0 - 1e-3
                    && v.pos[0] <= 10.0 + 1e-3
                    && v.pos[2] >= 8.0 - 1e-3
                    && v.pos[2] <= 9.0 + 1e-3
            });
            assert!(
                !on_seam_cell,
                "{name} mesh must cull the pillar's shared face at the seam"
            );
        }
    }

    let step_top_at = |wx: f32, wz: f32| vert_at(&lower_mesh.opaque, 0, [wx, 16.0, wz]);
    assert_eq!(
        ao_idx(step_top_at(9.0, 8.0)),
        2,
        "seam corner occluded from above"
    );
    assert_eq!(
        ao_idx(step_top_at(9.0, 9.0)),
        2,
        "seam corner occluded from above"
    );
    assert_eq!(ao_idx(step_top_at(8.0, 8.0)), 3, "open corner fully lit");
    assert_eq!(ao_idx(step_top_at(8.0, 9.0)), 3, "open corner fully lit");

    for (wx, wz) in [(8.0, 8.0), (8.0, 9.0), (9.0, 8.0), (9.0, 9.0)] {
        assert_eq!(
            light6(step_top_at(wx, wz)),
            63,
            "step top corner at ({wx}, {wz}) must take the upper section's skylight"
        );
    }

    let xface: Vec<&Vertex> = upper_mesh
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 2 && (v.pos[0] - 10.0).abs() < 1e-3)
        .collect();
    let bottom = xface
        .iter()
        .filter(|v| (v.pos[1] - 16.0).abs() < 1e-3)
        .map(|v| light6(v))
        .max()
        .expect("pillar +X face must have seam-level corners");
    let top = xface
        .iter()
        .filter(|v| (v.pos[1] - 17.0).abs() < 1e-3)
        .map(|v| light6(v))
        .min()
        .expect("pillar +X face must have upper corners");
    assert!(
        bottom < top,
        "seam-level corners ({bottom}) must blend the lower section's darkness (top {top})"
    );
}
