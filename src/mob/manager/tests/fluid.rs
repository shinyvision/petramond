use super::*;
use crate::entity::fluid_fixture::{self, block, flowing_brine, pool, BRINE, FLOOR_Y, SYRUP};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::exposure::ExposureSource;
use petramond_world::fluid::Buoyancy;

#[test]
fn fluid_rows_drive_players_and_mobs_through_the_world() {
    let root = fluid_fixture::stage("mob-fluids");
    crate::modding::tests::run_child_test(&root, "mob::manager::tests::fluid::fluid_bodies_inner");
}

fn body(name: &str, pos: WorldPos) -> Mobs {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(
        crate::mob::by_key(&format!("bodyfluid:{name}")).unwrap(),
        pos,
        0.0
    ));
    mobs
}

fn step(
    mobs: &mut Mobs,
    world: &World,
    drive: [f32; 2],
) -> super::super::simulation::MobTickEvents {
    assert!(mobs.set_mob_drive(0, Some(drive), None, None, false, false));
    mobs.tick(
        0.05,
        world,
        &[PlayerAnchor {
            pos: WorldPos::new(8.5, 76.0, 8.5),
            ..Default::default()
        }],
        false,
    )
}

#[test]
#[ignore = "child of fluid_rows_drive_players_and_mobs_through_the_world with fixture content"]
fn fluid_bodies_inner() {
    for name in [BRINE, SYRUP] {
        let world = pool(block(name), 75);
        equal_intent_gets_equal_response(name, &world);
        swimmers_bob_and_floaters_settle(name, &world);
    }
    a_current_carries_an_idle_mob();
    contact_respects_species_tolerance();
    tolerated_hazards_stay_routable();
}

fn a_current_carries_an_idle_mob() {
    let world = flowing_brine();
    let start = WorldPos::new(7.5, FLOOR_Y as f64, 8.5);
    let mut mobs = body("swim", start);
    for _ in 0..20 {
        step(&mut mobs, &world, [0.0, 0.0]);
    }
    assert!(
        mobs.instances()[0].pos.x > start.x + 0.1,
        "the current carries the body downstream: {:?}",
        mobs.instances()[0].pos
    );
}

fn equal_intent_gets_equal_response(name: &str, world: &World) {
    let start = WorldPos::new(5.5, 70.0, 8.5);
    let mut mobs = body("swim", start);
    let mut player = crate::player::Player::new(start);
    for _ in 0..20 {
        let events = step(&mut mobs, world, [crate::player::SWIM_SPEED, 0.0]);
        assert!(
            events.falls.is_empty(),
            "{name}: swimming accrues no impact"
        );
        player.update(
            0.05,
            world,
            crate::player::Input {
                wishdir: Vec3::X,
                jump: true,
                ..Default::default()
            },
        );
        let mob = &mobs.instances()[0];
        assert!(
            mob.pos.distance(player.pos) < 1e-4,
            "{name}: equal body intent gets equal fluid response: {:?} vs {:?}",
            mob.pos,
            player.pos
        );
    }
    let moved = mobs.instances()[0].pos - start;
    assert!(
        moved.x > 0.0 && moved.y > 0.0,
        "{name}: an immersed mob steers and swims up"
    );
}

fn swimmers_bob_and_floaters_settle(name: &str, world: &World) {
    let mut mobs = body("swim", WorldPos::new(8.5, 74.5, 8.5));
    let (mut up, mut down) = (false, false);
    for tick in 0..200 {
        let before = mobs.instances()[0].pos.y;
        let events = step(&mut mobs, world, [0.0, 0.0]);
        let after = mobs.instances()[0].pos.y;
        if tick > 100 {
            up |= after > before;
            down |= after < before;
        }
        assert!(
            (73.0..77.0).contains(&after),
            "{name}: swimmer stays at the surface: {after}"
        );
        assert!(events.falls.is_empty());
        assert!(
            events.splashes.is_empty(),
            "{name}: only splashing rows splash"
        );
    }
    assert!(up && down, "{name}: swimmers bob through the surface");

    let start = WorldPos::new(5.5, 70.0, 8.5);
    let mut neutral = body("neutral", start);
    for _ in 0..20 {
        step(&mut neutral, world, [0.0, 0.0]);
    }
    assert!(
        (neutral.instances()[0].pos.y - start.y).abs() < 1e-5,
        "{name}: neutral buoyancy has no automatic stroke"
    );

    let mut hull = body("surface", WorldPos::new(8.5, 74.5, 8.5));
    for _ in 0..200 {
        step(&mut hull, world, [0.0, 0.0]);
    }
    let y = hull.instances()[0].pos.y;
    for _ in 0..20 {
        step(&mut hull, world, [0.0, 0.0]);
    }
    assert!(
        (hull.instances()[0].pos.y - y).abs() < 1e-4,
        "{name}: surface buoyancy settles without bobbing"
    );
    assert!(world
        .body_fluid(hull.instances()[0].pos, 1.8, Buoyancy::Surface)
        .is_some());

    let mut dropped = body("surface", WorldPos::new(8.5, 90.0, 8.5));
    step(&mut dropped, world, [0.0, 0.0]);
    let falling = dropped.instances()[0].vel().y;
    step(&mut dropped, world, [0.0, 0.0]);
    assert!(
        dropped.instances()[0].vel().y < falling,
        "a floating body out of its fluid falls under gravity"
    );
}

/// Exposure a body of species `name` feels over two seconds in a syrup pool:
/// whether any hit arrived, whether one came from burning, and whether it
/// ever held burning.
fn soak(name: &str) -> (bool, bool, bool) {
    let burning = petramond_world::condition::by_name("petramond:burning").unwrap();
    let world = pool(block(SYRUP), 75);
    let mut mobs = body(name, WorldPos::new(8.5, 70.0, 8.5));
    let (mut hit, mut burned, mut held) = (false, false, false);
    for _ in 0..40 {
        let events = step(&mut mobs, &world, [0.0, 0.0]);
        hit |= !events.exposure.is_empty();
        burned |= events
            .exposure
            .iter()
            .any(|e| e.damage.source == ExposureSource::Condition(burning));
        held |= mobs.instances()[0]
            .exposure()
            .conditions()
            .get(burning)
            .is_some();
    }
    (hit, burned, held)
}

fn contact_respects_species_tolerance() {
    assert_eq!(
        soak("swim"),
        (true, true, true),
        "contact deals and applies"
    );
    assert_eq!(
        soak("dweller"),
        (false, false, false),
        "a tolerated block is never felt"
    );
    assert_eq!(
        soak("fireproof"),
        (true, false, false),
        "a tolerated condition is never held, while contact still hurts"
    );
}

fn tolerated_hazards_stay_routable() {
    let syrup = block(SYRUP);
    let mut world = pool(Block::Air, FLOOR_Y - 1);
    for x in 6..=9 {
        for z in 0..16 {
            world.set_block_world(x, FLOOR_Y - 1, z, syrup);
        }
    }
    let start = IVec3::new(2, FLOOR_Y, 8);
    let goal = IVec3::new(13, FLOOR_Y, 8);
    for (name, routable) in [("swim", false), ("dweller", true)] {
        let d = crate::mob::def(crate::mob::by_key(&format!("bodyfluid:{name}")).unwrap());
        assert_eq!(
            crate::mob::nav::destination_reachable(
                &world,
                start,
                goal,
                d.path_params(),
                d.size.height,
                None
            ),
            Some(routable),
            "{name}: only a tolerant species routes through a hazardous fluid"
        );
    }
}
