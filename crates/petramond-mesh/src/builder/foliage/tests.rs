use super::*;
use crate::face::quad_for;
use crate::vertex::pack_vertex;

#[test]
fn sprays_stay_in_the_air_neighbor_and_cull_margin() {
    for face in Face::ALL {
        let (dx, dy, _) = face.dir();
        let normal_axis = if dx != 0 {
            0
        } else if dy != 0 {
            1
        } else {
            2
        };
        for x in -32..32 {
            let seed = face_seed([x, -17, 31], face);
            let mut vertices: Vec<_> = quad_for(face, 0.0, 0.0, 0.0)
                .into_iter()
                .enumerate()
                .map(|(corner, pos)| Vertex {
                    pos,
                    tint: 0,
                    packed: pack_vertex(0, corner as u32, 0, false, 3, 63),
                    packed2: 0,
                })
                .collect();
            emit_spray(&mut vertices, 0, face, seed);
            for v in &vertices[4..] {
                for axis in 0..3 {
                    let margin = if axis == normal_axis {
                        FOLIAGE_OVERHANG
                    } else {
                        0.0
                    };
                    assert!(
                        (-margin..=1.0 + margin).contains(&v.pos[axis]),
                        "{face:?}: {:?}",
                        v.pos
                    );
                }
            }
            if vertices.len() > 4 {
                let p = |i: usize| Vec3::from_array(vertices[i].pos);
                let front = (p(5) - p(4)).cross(p(6) - p(4));
                let back = (p(9) - p(8)).cross(p(10) - p(8));
                assert!(front.length_squared() > 0.0);
                assert!(
                    front.dot(back) < 0.0,
                    "both windings must survive backface culling"
                );
            }
        }
    }
}

#[test]
fn spatial_seed_does_not_repeat_at_section_boundaries() {
    let mut seeds = std::collections::HashSet::new();
    for x in -64..64 {
        for face in Face::ALL {
            assert!(seeds.insert(face_seed([x * 16, -16, 16], face)));
        }
    }
}
