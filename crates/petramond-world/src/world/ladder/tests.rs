use super::*;
use crate::chunk::Chunk;
use crate::world::test_world::TestWorld;

fn world() -> TestWorld {
    TestWorld::with_chunk(0, Chunk::new(0, 0))
}

#[test]
fn a_ladder_is_supported_only_by_a_complete_wall_face() {
    let mut w = world();
    let ladder = IVec3::new(8, 64, 8);
    let wall = crate::ladder::support_cell(ladder, Facing::East);
    assert!(
        !w.data.ladder_supported_at(ladder, Facing::East),
        "no wall, no support"
    );
    w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
    assert!(w.data.ladder_supported_at(ladder, Facing::East));
    assert!(!w.data.ladder_supported_at(ladder, Facing::North));
}

#[test]
fn a_placed_ladder_collides_as_its_facing_resolved_panel() {
    let mut w = world();
    let p = IVec3::new(8, 64, 8);
    w.set_block_world(p.x, p.y, p.z, Block::LadderEast);
    let boxes = w.data.collision_boxes_at(p.x, p.y, p.z);
    assert_eq!(boxes, crate::ladder::collision_boxes(Facing::East));
    assert_eq!(boxes.len(), 1);
    let b = &boxes[0];
    assert!(b.max[0] - b.min[0] < 0.5 || b.max[2] - b.min[2] < 0.5);
    assert_eq!((b.min[1], b.max[1]), (0.0, 1.0));
}

#[test]
fn climbable_query_reads_the_facing_row() {
    let mut w = world();
    let p = IVec3::new(8, 64, 8);
    assert_eq!(w.data.climb_at(p.x, p.y, p.z), None);
    w.set_block_world(p.x, p.y, p.z, Block::LadderSouth);
    assert_eq!(
        w.data.climb_at(p.x, p.y, p.z),
        Some(Climb::Panel(Facing::South))
    );
    w.set_block_world(p.x, p.y, p.z, Block::Stone);
    assert_eq!(w.data.climb_at(p.x, p.y, p.z), None);
}
