use super::*;

const FULL: &[Aabb] = &[Aabb {
    min: [0.0; 3],
    max: [1.0; 3],
}];
const PANEL: &[Aabb] = &[Aabb {
    min: [0.0; 3],
    max: [0.0625, 1.0, 1.0],
}];
const SLAB: &[Aabb] = &[Aabb {
    min: [0.0; 3],
    max: [1.0, 0.5, 1.0],
}];

fn size() -> MobSize {
    MobSize {
        half_width: 0.3,
        height: 1.8,
        half_length: None,
    }
}

#[test]
fn straight_walk_refuses_walls_panels_and_damaging_drops_but_accepts_steps() {
    let at = WorldPos::new(0.5, 1.0, 0.5);
    let walk = |boxes: &dyn Fn(i32, i32, i32) -> &'static [Aabb]| {
        walk_clear(at, 0.0, size(), 1, [1.5, 0.0], 1.0, &boxes, &[], &|_, _| {
            true
        })
    };
    assert!(walk(&|_, y, _| if y == 0 { FULL } else { &[] }));
    assert!(!walk(&|x, y, _| if y == 0 || (x == 1 && y > 0 && y < 4) {
        FULL
    } else {
        &[]
    }));
    assert!(!walk(&|x, y, _| if y == 0 {
        FULL
    } else if x == 1 && y > 0 && y < 4 {
        PANEL
    } else {
        &[]
    }));
    assert!(!walk(
        &|x, y, _| if (x == 0 && y == 0) || (x != 0 && y == -5) {
            FULL
        } else {
            &[]
        }
    ));
    assert!(walk(&|x, y, _| if y == 0 {
        FULL
    } else if x > 0 && y == 1 {
        SLAB
    } else {
        &[]
    }));
    assert!(walk(
        &|x, y, _| if (x == 0 && y == 0) || (x > 0 && y == -1) {
            FULL
        } else {
            &[]
        }
    ));
}

#[test]
fn a_leg_cannot_cross_a_gap_or_ignore_a_dynamic_wall_or_unknown_terrain() {
    let at = WorldPos::new(0.5, 1.0, 0.5);
    let gap = |x, y, _| if y == 0 && x != 1 { FULL } else { &[] };
    assert!(!walk_clear(
        at,
        0.0,
        size(),
        1,
        [2.0, 0.0],
        1.0,
        &gap,
        &[],
        &|_, _| true
    ));
    let floor = |_, y, _| if y == 0 { FULL } else { &[] };
    let wall = [DynBox {
        min: [1.0, 1.0, -2.0],
        max: [1.5, 4.0, 2.0],
        id: 2,
    }];
    assert!(!walk_clear(
        at,
        0.0,
        size(),
        1,
        [1.5, 0.0],
        1.0,
        &floor,
        &wall,
        &|_, _| true
    ));
    assert!(!walk_clear(
        at,
        0.0,
        size(),
        1,
        [1.5, 0.0],
        1.0,
        &floor,
        &[],
        &|_, _| false
    ));
}

#[test]
fn the_world_probe_requires_ground_and_final_terrain() {
    use crate::mob::Mob;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos};

    let mut world = ServerWorld::new(0, 1);
    let mut chunk = Chunk::new(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    world.insert_empty_column_for_test(ChunkPos::new(0, 0));
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    let mut mob = Instance::new(Mob::Sheep, WorldPos::new(15.5, 64.0, 8.5), 0.0, 1);
    let offsets = [[-0.65, 0.0], [0.65, 0.0]];
    assert_eq!(probe(&world, &mob, &offsets, 1.0), [false, false]);
    mob.motion.on_ground = true;
    assert_eq!(
        probe(&world, &mob, &offsets, 1.0),
        [true, false],
        "loaded air above known ground is safe; an unknown neighbour is not"
    );
}
