use super::*;

#[test]
fn shared_corners_agree_across_section_boundaries() {
    let blocks = |_: i32, y, z| {
        if y == 15 && z == 0 {
            Block::OakLeaves
        } else {
            Block::Air
        }
    };
    for x in [-17, -1, 15, 31] {
        let left = CrownCorners::new([x, 15, 0], blocks, |_, _, _| true);
        let right = CrownCorners::new([x + 1, 15, 0], blocks, |_, _, _| true);
        for key in [0, 2, 4, 6] {
            assert_eq!(left.offsets[key + 1], right.offsets[key]);
            assert_ne!(left.offsets[key + 1], Vec3::ZERO);
        }
    }
}

#[test]
fn flat_canopies_and_solid_contacts_do_not_shrink() {
    let plane = CrownCorners::new(
        [0, 0, 0],
        |_, y, _| if y == 0 { Block::OakLeaves } else { Block::Air },
        |_, _, _| true,
    );
    assert!(plane.offsets.iter().all(|p| *p == Vec3::ZERO));
    let contact = CrownCorners::new(
        [0, 0, 0],
        |x, y, z| {
            if y == -1 {
                Block::Stone
            } else if [x, y, z] == [0, 0, 0] {
                Block::OakLeaves
            } else {
                Block::Air
            }
        },
        |_, _, _| true,
    );
    for key in [0, 1, 4, 5] {
        assert_eq!(contact.offsets[key], Vec3::ZERO);
    }
    let missing = CrownCorners::new(
        [0, 0, 0],
        |x, y, z| {
            if [x, y, z] == [0, 0, 0] {
                Block::OakLeaves
            } else {
                Block::Air
            }
        },
        |_, _, _| false,
    );
    assert!(missing.offsets.iter().all(|p| *p == Vec3::ZERO));
}

#[test]
fn subdivision_preserves_colored_light_and_uv_extent() {
    let light = BlockLight6::new(12, 35, 3);
    let mut vertices: Vec<_> = crate::face::quad_for(crate::face::Face::PosY, 0.0, 0.0, 0.0)
        .into_iter()
        .enumerate()
        .map(|(i, pos)| Vertex {
            pos,
            tint: light.tint_word([0.2, 0.5, 0.1]),
            packed: vertex::pack_vertex(0, i as u32, 0, false, 3, 42) | light.packed_bits(),
            packed2: light.packed2_bits() | vertex::DYED_FLAG2,
        })
        .collect();
    let shape = CrownCorners::new(
        [0, 0, 0],
        |x, y, z| {
            if [x, y, z] == [0, 0, 0] {
                Block::OakLeaves
            } else {
                Block::Air
            }
        },
        |_, _, _| true,
    );
    shape.soften_face(&mut vertices, 0, true);
    assert!(vertices.len() > 4);
    for v in &vertices {
        assert_eq!(vertex::decode_vertex_light(v), light);
        assert_eq!((v.packed >> vertex::SKY_SHIFT) & 63, 42);
        assert_ne!(v.packed2 & vertex::DYED_FLAG2, 0);
        assert!(v.pos.iter().all(|&p| (0.0..=1.0).contains(&p)));
    }
    let uv = |v: &Vertex| {
        (
            (v.packed2 >> vertex::CELL_UV_U_SHIFT) & vertex::CELL_UV_MASK,
            (v.packed2 >> vertex::CELL_UV_V_SHIFT) & vertex::CELL_UV_MASK,
        )
    };
    for corner in [(0, 0), (16, 0), (0, 16), (16, 16)] {
        assert!(vertices.iter().any(|v| uv(v) == corner));
    }
    let colors = [
        BlockLight6::new(63, 0, 0),
        BlockLight6::new(0, 63, 0),
        BlockLight6::new(0, 0, 63),
        BlockLight6::new(0, 0, 0),
    ];
    let varied = colors.map(|light| Vertex {
        pos: [0.0; 3],
        tint: light.tint_word([0.2, 0.5, 0.1]),
        packed: light.packed_bits(),
        packed2: light.packed2_bits(),
    });
    assert_eq!(
        vertex::decode_vertex_light(&sample(&varied, [1; 4], Vec3::ZERO, 8, 8)),
        BlockLight6::new(16, 16, 16)
    );
}
