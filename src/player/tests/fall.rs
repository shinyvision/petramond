use super::*;

#[test]
fn fall_distance_measures_the_drop_height() {
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let mut pl = p(WorldPos::new(0.5, 68.0, 0.5));
    for _ in 0..240 {
        pl.update_core(1.0 / 60.0, &solid, Input::default());
    }
    assert!(pl.on_ground, "player has landed");
    assert!((pl.pos.y - 64.0).abs() < 1e-3, "feet on the floor top");
    let dist = pl.take_fall_distance();
    assert!(
        (dist - 4.0).abs() < 0.05,
        "a 4-block drop measures ~4 blocks, got {dist}"
    );
    assert_eq!(pl.take_fall_distance(), 0.0);
}

#[test]
fn walking_off_a_low_ledge_measures_a_short_fall() {
    let solid = |x: i32, y: i32, _z: i32| if x <= 0 { y < 64 } else { y < 62 };
    let walk = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut pl = p(WorldPos::new(0.5, 64.0, 0.5));
    for _ in 0..240 {
        pl.update_core(1.0 / 60.0, &solid, walk);
    }
    assert!(pl.on_ground);
    let dist = pl.take_fall_distance();
    assert!(
        dist < 3.0,
        "a 2-block step-down stays under the safe distance, got {dist}"
    );
}

#[test]
fn a_fluid_cancels_the_fall() {
    let mut chunk = petramond_world::chunk::Chunk::new(0, 0);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set_block(x, 59, z, Block::Stone);
            for y in 60..=71 {
                chunk.set_block(x, y, z, Block::Water);
            }
        }
    }
    let mut world = crate::world::ServerWorld::new(0, 1);
    world.insert_chunk_for_test(petramond_world::chunk::ChunkPos::new(0, 0), chunk);
    let mut pl = p(WorldPos::new(8.5, 70.0, 8.5));
    for _ in 0..1200 {
        pl.update(1.0 / 60.0, world.data(), Input::default());
    }
    let dist = pl.take_fall_distance();
    assert!(
        dist < 3.0,
        "immersion should break the fall (no damage), measured {dist}"
    );
}

#[test]
fn mode_switch_drops_a_pending_fall() {
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let mut pl = p(WorldPos::new(0.5, 70.0, 0.5));
    for _ in 0..240 {
        pl.update_core(1.0 / 60.0, &solid, Input::default());
    }
    pl.toggle_mode();
    assert_eq!(
        pl.take_fall_distance(),
        0.0,
        "mode switch clears the landing"
    );
}
