use super::{plan_now, AT};
use crate::host::fake::rows::{DIRT, STONE};
use crate::host::prelude::*;
use crate::testing::{Session, CHEST_AT, HOME};
use crate::worker::{Step, Task, Then};

#[test]
fn a_full_inventory_can_be_spent_on_nearby_work_before_unloading() {
    let (mut session, id) = Session::row();
    session.world.set([2, 0, 0], DIRT);
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    let hands = ContainerAddress::Entity(golem);
    session.world.give(hands, "petramond:stone", 2);
    while session.world.container(hands).iter().any(Option::is_none) {
        session.world.give(hands, "petramond:raw_copper", 64);
    }
    let step = plan_now(&mut session, id, golem);
    assert!(
        matches!(
            step,
            Step::Centre {
                task: Task::Unit(_),
                ..
            }
        ),
        "use the carried blocks before a chest trip: {step:?}"
    );
}

#[test]
fn collecting_digs_still_require_room_when_no_carried_placement_can_help() {
    let (mut session, id) = Session::row();
    session.world.fill([0, 0, 0], [2, 0, 0], DIRT);
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    let hands = ContainerAddress::Entity(golem);
    while session.world.container(hands).iter().any(Option::is_none) {
        session.world.give(hands, "petramond:raw_copper", 64);
    }
    let step = plan_now(&mut session, id, golem);
    assert!(
        matches!(
            step,
            Step::Walk {
                then: Then::Deposit(_),
                ..
            }
        ),
        "make room for the selected digging work: {step:?}"
    );
}

#[test]
fn a_tool_for_later_digging_does_not_interrupt_usable_nearby_blocks() {
    for clear_x in [1, 2] {
        let (mut session, id) = Session::row();
        session.world.set([clear_x, 0, 0], DIRT);
        session.job(id);
        let golem = session.golem(id, HOME, AT);
        session
            .world
            .give(ContainerAddress::Entity(golem), "petramond:stone", 2);
        session.world.give(
            ContainerAddress::Block(CHEST_AT),
            "petramond:stone_shovel",
            1,
        );
        let step = plan_now(&mut session, id, golem);
        assert!(
            matches!(
                step,
                Step::Centre {
                    task: Task::Unit(_),
                    ..
                }
            ),
            "place nearby blocks before fetching a future tool: {step:?}"
        );
        assert_eq!(
            session.builder.jobs.map[&id].crew.cargo.tool_trip.at, 0,
            "only an actual tool trip starts its cooldown"
        );
        let Step::Centre {
            task: Task::Unit(i),
            ..
        } = step
        else {
            unreachable!()
        };
        assert!(matches!(
            session.builder.jobs.map[&id].survey.as_ref().unwrap().known[i],
            crate::survey::Known::Place(_)
        ));
    }
}

#[test]
fn a_nearby_tool_trip_can_win_when_it_makes_the_current_digging_cheaper() {
    let (mut session, id) = Session::row();
    session.world.fill([0, 0, 0], [2, 0, 0], DIRT);
    session.world.state_mut().block_rows[usize::from(DIRT.0)]
        .info
        .hardness = 2.0;
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    session.world.give(
        ContainerAddress::Block(CHEST_AT),
        "petramond:stone_shovel",
        1,
    );
    let step = plan_now(&mut session, id, golem);
    assert!(
        matches!(
            step,
            Step::Walk {
                then: Then::Fetch(_),
                ..
            }
        ),
        "compare the tool's time saving with its chest detour: {step:?}"
    );
    assert!(session.builder.jobs.map[&id].crew.cargo.tool_trip.at > 0);
}

#[test]
fn a_scaffold_request_does_not_interrupt_work_that_needs_no_scaffolding() {
    let (mut session, id) = Session::row();
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    session
        .world
        .give(ContainerAddress::Entity(golem), "petramond:stone", 3);
    session
        .world
        .give(ContainerAddress::Block(CHEST_AT), "petramond:dirt", 64);
    session
        .builder
        .jobs
        .map
        .get_mut(&id)
        .unwrap()
        .crew
        .scaffolding
        .short = true;
    let step = plan_now(&mut session, id, golem);
    assert!(
        matches!(
            step,
            Step::Centre {
                task: Task::Unit(_),
                ..
            }
        ),
        "do the available ground work before collecting scaffolding: {step:?}"
    );
    assert!(
        session.builder.jobs.map[&id].crew.scaffolding.short,
        "a postponed scaffold request must survive"
    );
}

#[test]
fn the_chest_detour_competes_with_the_walk_to_carried_work() {
    for nearby_chest in [false, true] {
        let (mut session, id) = Session::site(
            "Two materials",
            [25, 1, 1],
            &[
                ([0, 0, 0], "petramond:oak_planks"),
                ([24, 0, 0], "petramond:stone"),
            ],
        );
        session.world.fill([-32, -1, -16], [32, -1, 16], STONE);
        let table = if nearby_chest {
            [0, 0, -3]
        } else {
            [-24, 0, -3]
        };
        let chest = [table[0] + 1, 0, -3];
        session.world.set(table, crate::host::fake::rows::TABLE);
        session.world.chest(chest, 8);
        session.builder.projects.update(id, |p| p.table = table);
        session.job(id);
        let golem = session.golem(id, HOME, AT);
        session
            .world
            .give(ContainerAddress::Entity(golem), "petramond:stone", 1);
        session
            .world
            .give(ContainerAddress::Block(chest), "petramond:oak_planks", 64);
        let step = plan_now(&mut session, id, golem);
        let expected = if nearby_chest {
            "fetch to unlock the nearby work"
        } else {
            "walk to work using carried materials"
        };
        assert!(
            matches!(
                step,
                Step::Walk {
                    then: Then::Fetch(_),
                    ..
                }
            ) == nearby_chest,
            "{expected}: {step:?}"
        );
        assert!(
            matches!(
                step,
                Step::Walk {
                    then: Then::Task(_),
                    ..
                }
            ) != nearby_chest,
            "{expected}: {step:?}"
        );
    }
}
