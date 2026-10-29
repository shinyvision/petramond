use super::*;
use crate::mob::kinematics::{Locomotion, Surroundings};
use crate::mob::{def, Instance};
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, SectionPos};
use petramond_world::section::Section;

#[test]
fn jump_edges_agree_with_motion_at_asymmetric_partial_obstacles() {
    let root = crate::modding::tests::stage_mods_fixture("partial-jump", &[]).unwrap();
    let pack = root.join("mods/jumpfixture");
    std::fs::create_dir_all(&pack).unwrap();
    let write = |file: &str, data: serde_json::Value| {
        std::fs::write(pack.join(file), data.to_string()).unwrap();
    };
    write(
        "pack.json",
        serde_json::json!({"id":"jumpfixture", "name":"Partial Jump Fixture", "version":"0.0.1"}),
    );
    write(
        "blocks.json",
        serde_json::json!({"blocks": [
            row("lower", serde_json::json!([
                {"min":[0,0,0], "max":[1,0.5,1]},
                {"min":[0,0,0], "max":[1,1,0.125]}
            ])),
            row("upper", serde_json::json!([
                {"min":[0,0,0], "max":[1,0.3,0.125]}
            ]))
        ]}),
    );
    write(
        "mobs.json",
        serde_json::json!({"mobs":[{
            "mob":"jumpfixture:walker", "key":"jumpfixture:walker", "model":"models/owl.bbmodel", "scale":1,
            "size":{"half_width":0.3,"height":1.5}, "tags":{"petramond:health":20},
            "walk_speed":3.4,"jump_speed":7.2,"turn_rate":8,"walk_anim_rate":1,
            "cap":1,"spawn_group":{"min":1,"max":1},
            "wander":{"chance_per_tick":0,"radius":1},"habitat":{"avoid":[],"prefer":[]},"avoid_fluids":false,"brain":[]
        }]}),
    );
    crate::modding::tests::with_fixture_content(&root, check_edges);
}

fn row(name: &str, boxes: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "block":format!("jumpfixture:{name}"),"shape":"cube","flags":["solid"],"tags":[],
        "collision":boxes,"emission":0,"tiles":["stone","stone","stone"],
        "material":"stone","hardness":1,"drops":[]
    })
}

fn check_edges() {
    let kind = crate::mob::by_key("jumpfixture:walker").unwrap();
    let d = def(kind);
    let mut world = crate::world::ServerWorld::new(0, 1);
    let mut chunk = Chunk::new(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    world.insert_section_for_test(SectionPos::new(0, 4, 0), Section::new(0, 4, 0));
    let block = |name: &str| serde_json::from_value::<Block>(serde_json::json!(name)).unwrap();
    world.set_block_world(7, 64, 8, block("jumpfixture:lower"));
    world.set_block_world(7, 65, 8, block("jumpfixture:upper"));
    let rear = IVec3::new(7, 64, 7);
    let front = IVec3::new(7, 64, 9);
    let top = IVec3::new(7, 65, 8);
    let cursor = world.cursor();
    let boxes = |x, y, z| cursor.collision_boxes_xyz(x, y, z);
    let mut body = Instance::new(kind, WorldPos::new(7.5, 64.0, 7.5), std::f32::consts::PI, 1);
    for _ in 0..60 {
        body.integrate_locomotion(
            0.05,
            d,
            Locomotion {
                wish: Vec3::Z,
                jump: body.on_ground(),
                can_steer: true,
            },
            &Surroundings::dry(&boxes),
        );
    }
    assert!(
        body.pos.z < 8.0,
        "the body cannot clear the high entering face: {:?}",
        body.pos
    );
    let allowed = navigation_step_gate(&cursor, d.path_params(), d.size.height);
    assert!(
        !allowed(rear, top),
        "a jump may not gain an extra auto-step above its landing height"
    );
    assert!(
        allowed(front, top),
        "the low entering face must remain usable"
    );
    assert!(
        !allowed(top, rear),
        "the back outside the standing footprint cannot lift the starting feet"
    );
    let mut body = Instance::new(kind, WorldPos::new(7.5, 64.0, 9.5), 0.0, 2);
    for _ in 0..60 {
        let wish = if body.pos.z > 8.5 {
            Vec3::NEG_Z
        } else {
            Vec3::ZERO
        };
        body.integrate_locomotion(
            0.05,
            d,
            Locomotion {
                wish,
                jump: false,
                can_steer: true,
            },
            &Surroundings::dry(&boxes),
        );
    }
    assert!(
        body.on_ground() && body.pos.y > f64::from(front.y) && body.pos.z < f64::from(front.z),
        "the admitted low side must actually be traversable: {:?}",
        body.pos
    );
    let route = crate::mob::nav::route_path(&world, kind, rear, front);
    assert_eq!(
        route.last(),
        Some(&front),
        "a physical detour remains available: {route:?}"
    );
    assert!(
        !route.windows(2).any(|edge| edge == [rear, top]),
        "routing must use the detour: {route:?}"
    );
}
