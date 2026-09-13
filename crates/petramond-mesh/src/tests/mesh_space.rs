//! Mesh space: a section's vertices are column-local, so identical content
//! meshes to identical positions however far from the world origin it lies.

use super::*;

/// Positions only: tile variation legitimately hashes the world coordinate,
/// so the packed words may differ between the two copies.
#[test]
fn a_far_section_meshes_to_the_same_positions_as_one_at_spawn() {
    let blocks = [
        ((8usize, 8usize, 8usize), Block::FurnitureWorkbench),
        ((8, 7, 8), Block::Stone),
        ((9, 7, 8), Block::Stone),
        ((3, 8, 3), Block::StoneSlab),
        ((5, 7, 5), Block::Stone),
        ((5, 8, 5), Block::ShortGrass),
    ];
    let mesh_at = |pos: SectionPos| {
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        for &((x, y, z), b) in &blocks {
            section.set_block(x, y, z, b);
        }
        let (ox, oy, oz) = pos.origin_world();
        mesh_in_scene(
            &section,
            pos,
            |wx, wy, wz| {
                let (x, y, z) = (wx - ox, wy - oy, wz - oz);
                if in_section(x, y, z) {
                    section.block_raw(x as usize, y as usize, z as usize)
                } else {
                    Block::Air.id()
                }
            },
            |_, _, _| SKY_FULL,
        )
    };
    // A billion blocks out, where an absolute f32 coordinate rounds to 64.
    let far = 62_500_000;
    let near = mesh_at(SectionPos::new(0, 4, 0));
    let far = mesh_at(SectionPos::new(far, 4, -far));
    assert!(
        !near.model.is_empty() && !near.contact.is_empty(),
        "the scene must exercise the model and contact streams"
    );
    let positions = |verts: &[Vertex]| verts.iter().map(|v| v.pos).collect::<Vec<_>>();
    assert_eq!(positions(&near.opaque), positions(&far.opaque));
    assert_eq!(
        near.model.iter().map(|v| v.pos).collect::<Vec<_>>(),
        far.model.iter().map(|v| v.pos).collect::<Vec<_>>()
    );
    assert_eq!(
        near.contact.iter().map(|v| v.pos).collect::<Vec<_>>(),
        far.contact.iter().map(|v| v.pos).collect::<Vec<_>>()
    );
}
