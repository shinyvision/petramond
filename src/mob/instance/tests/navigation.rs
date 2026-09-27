use super::*;
use crate::entity::fluid_fixture;
use crate::mob::brain::AiBehavior;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, SectionPos};
use petramond_world::section::Section;

struct Goal(IVec3);

impl AiBehavior for Goal {
    fn tick(&mut self, _: &mut AiCtx) -> BehaviorOutput {
        BehaviorOutput {
            goal: Some(self.0),
            ..Default::default()
        }
    }
}

#[test]
fn walking_bodies_avoid_hazards_and_escape_them() {
    fluid_fixture::with_content("hazard-walk", hazard_walk_inner);
}

fn hazard_walk_inner() {
    a_walking_mob_detours_around_a_hazard_and_stops_when_it_cuts_off_the_route();
    a_mob_in_a_hazard_keeps_swimming_until_it_reaches_the_shore();
}

fn swimmer() -> Mob {
    crate::mob::by_key("bodyfluid:swim").unwrap()
}

fn a_walking_mob_detours_around_a_hazard_and_stops_when_it_cuts_off_the_route() {
    let syrup = fluid_fixture::block(fluid_fixture::SYRUP);
    let mut world = world();
    for z in 5..=10 {
        world.set_block_world(7, 63, z, syrup);
    }
    let goal = IVec3::new(13, 64, 8);
    let mut mob = Instance::new(swimmer(), WorldPos::new(2.5, 64.0, 8.5), 0.0, 1);
    mob.mind.brain = Brain::new().with_boxed(0, Box::new(Goal(goal)));
    let anchors = [PlayerAnchor {
        pos: WorldPos::new(8.5, 66.0, 8.5),
        ..Default::default()
    }];
    let mut regions = confined::RegionCache::default();
    let mut reached = false;
    for _ in 0..500 {
        tick(&mut mob, &world, &anchors, &mut regions);
        assert!(
            crate::exposure::touched_fluids(&world, [mob.aabb()]).is_some_and(|f| f.is_empty()),
            "walking must stay clear of the pool: {:?}",
            mob.pos
        );
        if mob.pos.x > 12.8 {
            reached = true;
            break;
        }
    }
    assert!(
        reached,
        "must walk the detour instead of stalling: {:?}",
        mob.pos
    );

    mob.mind.brain = Brain::new().with_boxed(0, Box::new(Goal(IVec3::new(2, 64, 8))));
    tick(&mut mob, &world, &anchors, &mut regions);
    for z in 0..16 {
        world.set_block_world(11, 63, z, syrup);
    }
    for _ in 0..150 {
        tick(&mut mob, &world, &anchors, &mut regions);
        assert!(
            mob.pos.x >= 12.0,
            "must stop on the near bank: {:?}",
            mob.pos
        );
        assert!(crate::exposure::touched_fluids(&world, [mob.aabb()]).is_some_and(|f| f.is_empty()));
    }
}

fn a_mob_in_a_hazard_keeps_swimming_until_it_reaches_the_shore() {
    let syrup = fluid_fixture::block(fluid_fixture::SYRUP);
    let mut world = world();
    for x in 4..=9 {
        for z in 0..16 {
            world.set_block_world(x, 63, z, syrup);
        }
    }
    let mut mob = Instance::new(swimmer(), WorldPos::new(6.5, 63.4, 8.5), 0.0, 1);
    mob.mind.brain = Brain::new().with_boxed(0, Box::new(Goal(IVec3::new(13, 64, 8))));
    let anchors = [PlayerAnchor {
        pos: WorldPos::new(8.5, 66.0, 8.5),
        ..Default::default()
    }];
    let mut regions = confined::RegionCache::default();
    for _ in 0..600 {
        tick(&mut mob, &world, &anchors, &mut regions);
        if mob.pos.x > 11.0 && mob.motion.on_ground {
            assert!(
                crate::exposure::touched_fluids(&world, [mob.aabb()]).is_some_and(|f| f.is_empty())
            );
            return;
        }
    }
    panic!("the hazard gate must allow swimming ashore: {:?}", mob.pos);
}

#[test]
fn routes_and_bodies_climb_out_of_every_fluid_onto_a_one_block_bank() {
    fluid_fixture::with_content("shore-climb", shore_climb_inner);
}

fn shore_climb_inner() {
    const TOP: i32 = 67;
    const BANK_X: i32 = 10;
    let dweller = crate::mob::by_key("bodyfluid:dweller").unwrap();
    for name in [fluid_fixture::BRINE, fluid_fixture::SYRUP] {
        for (rise, climbable) in [(0, true), (1, true), (3, false)] {
            let mut world = fluid_fixture::pool(fluid_fixture::block(name), TOP);
            fluid_fixture::bank(&mut world, BANK_X, TOP + rise);
            let stand_y = (TOP + rise + 1) as f32;
            let ashore = |pos: WorldPos, on_ground: bool| {
                on_ground && pos.x > BANK_X as f64 && pos.y >= f64::from(stand_y - 1e-3)
            };
            let start = fluid_fixture::beside_bank(BANK_X, TOP);

            let goal = IVec3::new(13, TOP + rise + 1, 8);
            let mut mob = Instance::new(dweller, start, 0.0, 1);
            mob.mind.brain = Brain::new().with_boxed(0, Box::new(Goal(goal)));
            let anchors = [PlayerAnchor {
                pos: WorldPos::new(8.5, f64::from(stand_y), 8.5),
                ..Default::default()
            }];
            let mut regions = confined::RegionCache::default();
            let mut mob_ashore = false;
            for _ in 0..400 {
                tick(&mut mob, &world, &anchors, &mut regions);
                mob_ashore |= ashore(mob.pos, mob.motion.on_ground);
            }
            assert_eq!(
                mob_ashore, climbable,
                "{name}, bank {rise} above the surface cell: mob ended at {:?}",
                mob.pos
            );

            let mut player = crate::player::Player::new(start);
            let climb = crate::player::Input {
                wishdir: Vec3::X,
                jump: true,
                ..Default::default()
            };
            let mut player_ashore = false;
            for _ in 0..1200 {
                player.update(1.0 / 60.0, world.data(), climb);
                player_ashore |= ashore(player.pos, player.on_ground);
            }
            assert_eq!(
                player_ashore, climbable,
                "{name}, bank {rise} above the surface cell: player ended at {:?}",
                player.pos
            );
        }
    }
}

fn world() -> ServerWorld {
    let mut chunk = Chunk::new(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    let mut world = ServerWorld::new(0, 1);
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    world.insert_section_for_test(SectionPos::new(0, 4, 0), Section::new(0, 4, 0));
    world
}

fn tick(
    mob: &mut Instance,
    world: &ServerWorld,
    anchors: &[PlayerAnchor],
    regions: &mut confined::RegionCache,
) {
    let inputs = TickInputs {
        world,
        players: anchors,
        noises: crate::mob::NoiseField::empty(),
        mobs: crate::mob::spatial::MobSnapshot::empty(),
        path_budget: None,
        solid: &[],
        solid_escape: &[],
    };
    let meta = SpeciesMeta::default();
    let ctx = MobTickCtx {
        dt: 0.05,
        inputs: &inputs,
        anchor: &anchors[0],
        def: crate::mob::def(mob.kind),
        meta: &meta,
    };
    mob.tick_alone(&ctx, regions, true);
}

#[test]
fn a_body_walking_down_a_flight_of_ledges_lands_on_every_one() {
    let mut world = world();
    for (x, top) in [(2, 68), (3, 68), (4, 67), (5, 66), (6, 65), (7, 64)] {
        for y in 64..top {
            for z in 7..=9 {
                world.set_block_world(x, y, z, Block::Stone);
            }
        }
    }
    let goal = IVec3::new(11, 64, 8);
    struct Hurry(IVec3);
    impl AiBehavior for Hurry {
        fn tick(&mut self, _: &mut AiCtx) -> BehaviorOutput {
            BehaviorOutput {
                goal: Some(self.0),
                speed_scale: Some(1.6),
                ..Default::default()
            }
        }
    }
    let mut mob = Instance::new(Mob::Owl, WorldPos::new(2.5, 68.0, 8.5), 0.0, 1);
    mob.mind.brain = Brain::new().with_boxed(0, Box::new(Hurry(goal)));
    let anchors = [PlayerAnchor {
        pos: WorldPos::new(8.5, 70.0, 8.5),
        ..Default::default()
    }];
    let mut regions = confined::RegionCache::default();
    let mut left_at = mob.pos.y;
    let mut landings = 0;
    for _ in 0..400 {
        let was_grounded = mob.motion.on_ground;
        tick(&mut mob, &world, &anchors, &mut regions);
        if was_grounded && !mob.motion.on_ground {
            left_at = mob.pos.y;
        }
        if !was_grounded && mob.motion.on_ground {
            landings += 1;
            assert!(
                left_at - mob.pos.y < 1.2,
                "fell {} in one go, past a landing, at {:?}",
                left_at - mob.pos.y,
                mob.pos
            );
        }
        if mob.pos.x > 10.8 {
            break;
        }
    }
    assert!(
        mob.pos.x > 10.8,
        "walks the whole flight down: {:?}",
        mob.pos
    );
    assert!(landings >= 4, "one landing a step: {landings}");
}
