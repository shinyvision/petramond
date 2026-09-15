use super::*;
use crate::entity::fluid_fixture::{self, FLOOR_Y};
use crate::events::{with_sessions_scope, PostQueue, SessionPlayerRef, SimCtx};
use mod_api::{ConditionId, EntityRef, HostRet, PlayerId};
use petramond_math::world_pos::WorldPos;

fn run_with_other(sim: &mut Sim, other: &mut Player) {
    let mut other_gui = petramond_world::gui_state::empty_gui_state();
    with_sessions_scope(
        (crate::player::PlayerId(0), 0),
        None,
        vec![SessionPlayerRef {
            id: crate::player::PlayerId(1),
            index: 1,
            player: other,
            gui_state: &mut other_gui,
            gui: None,
        }],
        || sim.run_slot(Attach::Before(Stage::Mining)),
    );
}

#[test]
fn a_guest_applies_and_cools_a_condition_on_the_addressed_player_and_mob() {
    let burning = petramond_world::condition::by_name("petramond:burning").unwrap();
    let def = burning.def();
    let strongest = (def.stages.len() - 1) as u8;
    let condition = ConditionId(burning.0);
    let mut sim = Sim::new();
    let mut other = Player::new(WorldPos::new(4.0, 80.0, 0.0));
    let mob = sim
        .world
        .spawn_mob(crate::mob::Mob::Owl, WorldPos::new(8.0, 80.0, 0.0), 0.0)
        .unwrap();
    let mut host = ModHost::from_instances(vec![calling_guest(
        "condition_test",
        &[EntityRef::Player(PlayerId(1)), EntityRef::Mob(mob)].map(|entity| {
            HostCall::EntityConditionApply {
                entity,
                condition,
                stage: strongest,
                ticks: 80,
            }
        }),
    )]);
    sim.init(&mut host);
    run_with_other(&mut sim, &mut other);
    assert!(!host.probe(0).0, "the real guest completed both host calls");
    assert!(
        sim.player.conditions().active().is_empty(),
        "the host player was not addressed"
    );
    let mob_conditions = sim.world.mobs().instances()[0].exposure().conditions();
    assert_eq!(other.conditions().active(), mob_conditions.active());
    assert_eq!(other.conditions().get(burning).unwrap().stage(), strongest);
    assert!(other.effects().is_empty());

    let mut cool = ModHost::from_instances(vec![calling_guest(
        "cool_test",
        &[EntityRef::Mob(mob), EntityRef::Player(PlayerId(1))].map(|entity| {
            HostCall::EntityConditionCool {
                entity,
                condition,
                ticks: u32::MAX,
            }
        }),
    )]);
    // A fresh system list runs only the cooling producer.
    sim.systems = TickSystems::default();
    sim.init(&mut cool);
    run_with_other(&mut sim, &mut other);
    assert!(!cool.probe(0).0);
    assert!(other.conditions().active().is_empty());
    assert!(sim.world.mobs().instances()[0]
        .exposure()
        .conditions()
        .active()
        .is_empty());
}

const HOT: &str = "condfix:hot";
const DOUSE: &str = "condfix:douse";

#[test]
fn a_body_in_a_clearing_fluid_refuses_a_grant_after_its_exposure_tick() {
    let Some(root) = stage_mods_fixture("condition-douse", &[]) else {
        return;
    };
    let pack = root.join("mods/condfix");
    std::fs::create_dir_all(&pack).unwrap();
    let write = |file: &str, value: serde_json::Value| {
        std::fs::write(pack.join(file), value.to_string()).unwrap();
    };
    write(
        "pack.json",
        serde_json::json!({"id": "condfix", "name": "Condition Fixture", "version": "0.0.1"}),
    );
    write(
        "conditions.json",
        serde_json::json!({"conditions": [{"condition": HOT, "stages": [{"stage": "on"}]}]}),
    );
    write(
        "blocks.json",
        serde_json::json!({"blocks": [fluid_fixture::fluid_row(
            DOUSE, "jump", 1.0, 0.5, 0.0, serde_json::json!({"clears": [HOT]}), &[], 0.0,
        )]}),
    );
    run_child_test(&root, "modding::tests::conditions::doused_grant_inner");
}

/// Mobs tick exposure before a mod's later stage runs, so a mod granting a
/// condition to a body standing in a fluid that clears it must be refused, or
/// the condition would flicker on for one tick.
#[test]
#[ignore = "child of a_body_in_a_clearing_fluid_refuses_a_grant_after_its_exposure_tick"]
fn doused_grant_inner() {
    let hot = petramond_world::condition::by_name(HOT).unwrap();
    let douse = fluid_fixture::block(DOUSE);
    let mut world = fluid_fixture::pool(douse, FLOOR_Y + 2);
    let feet = WorldPos::new(8.5, FLOOR_Y as f64, 8.5);
    let mob = world.spawn_mob(crate::mob::Mob::Sheep, feet, 0.0).unwrap();
    let mut store = super::super::host::ModStoreData::new("condfix", 1);
    let mut grant = |world: &mut World| {
        let mut player = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let (mut feed, mut queue) = (TickEvents::default(), PostQueue::default());
        let mut gui = petramond_world::gui_state::empty_gui_state();
        let mut ctx = SimCtx {
            world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        let mut reply = HostRet::Unit;
        super::super::scope::enter(&mut ctx, || {
            reply = super::super::host::handle_host_call(
                &mut store,
                HostCall::EntityConditionApply {
                    entity: EntityRef::Mob(mob),
                    condition: ConditionId(hot.0),
                    stage: 0,
                    ticks: 100,
                },
            );
        });
        reply
    };
    let held = |world: &World| {
        world.mobs().instances()[0]
            .exposure()
            .conditions()
            .get(hot)
            .is_some()
    };
    let anchors = [crate::mob::PlayerAnchor {
        pos: feet,
        ..Default::default()
    }];

    assert_eq!(
        grant(&mut world),
        HostRet::Bool(true),
        "no contact seen yet"
    );
    world.tick_mobs(crate::events::tick::TICK_DT, &anchors);
    assert!(!held(&world), "the exposure tick cleared it");
    assert_eq!(grant(&mut world), HostRet::Bool(false));
    assert!(!held(&world), "a later grant cannot relight it");
    world.tick_mobs(crate::events::tick::TICK_DT, &anchors);
    assert_eq!(
        grant(&mut world),
        HostRet::Bool(false),
        "still in the fluid"
    );

    for y in FLOOR_Y..=FLOOR_Y + 2 {
        for z in 0..16 {
            for x in 0..16 {
                world.set_block_world(x, y, z, petramond_world::block::Block::Air);
            }
        }
    }
    world.tick_mobs(crate::events::tick::TICK_DT, &anchors);
    assert_eq!(grant(&mut world), HostRet::Bool(true), "out of the fluid");
    assert!(held(&world));
}
