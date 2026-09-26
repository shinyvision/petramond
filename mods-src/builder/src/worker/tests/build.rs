//! What the golem builds with and on: its scaffolding, supports under work
//! with nothing beside it, pockets it must not seal, pillars, and tools.

use std::collections::BTreeMap;

use super::{at_work, unit, working};
use crate::fx::HashSet;
use crate::geometry::{feet_of, reaches};
use crate::host::fake::rows::{AIR, CHEST, DIRT, STONE};
use crate::host::fake::stack;
use crate::host::prelude::*;
use crate::survey::ItemKey;
use crate::testing::{Session, CHEST_AT, HOME};
use crate::worker::stance::Search;
use crate::worker::support::{self, Support};
use crate::worker::{cargo, pillar, pocket, scaffold, Task};

const AT: [i32; 3] = [1, 0, -2];

fn key(item: &str) -> ItemKey {
    (item.into(), Vec::new())
}

#[test]
fn scaffolding_is_what_is_most_spare_in_hand_softest_first() {
    let (mut session, id, golem) = working(AT);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:dirt", 10);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:oak_planks", 5);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        let picked = scaffold::pick(ctx, job, &body).map(|k| k.item.clone());
        assert_eq!(picked.as_deref(), Some("petramond:dirt"));
        assert_eq!(scaffold::in_hand(ctx, job, &body.slots), 15);
        assert_eq!(
            scaffold::record(ctx, job, &body).map(|r| r.block),
            Some("petramond:dirt".to_owned())
        );
        assert_eq!(
            scaffold::tools(ctx, &body.slots, None),
            vec!["shovel".to_owned(), "axe".to_owned()],
            "and the tools that take it down again"
        );
    });
    drop(session);

    let (mut session, id, golem) = working(AT);
    for item in ["petramond:oak_planks", "petramond:dirt"] {
        session.world.give(ContainerAddress::Mob(golem), item, 5);
    }
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        let picked = scaffold::pick(ctx, job, &body).map(|k| k.item.clone());
        assert_eq!(picked.as_deref(), Some("petramond:dirt"), "as spare: the softer");
    });
}

#[test]
fn scaffolding_is_fetched_up_to_a_slots_worth_or_the_pillars_height() {
    let (mut session, id, _) = working(AT);
    let stock: BTreeMap<ItemKey, u32> = [
        (key("petramond:dirt"), 100),
        (key("petramond:oak_planks"), 20),
        (key("petramond:stone"), 50),
    ]
    .into_iter()
    .collect();
    at_work(&mut session, id, |ctx, _, job| {
        assert_eq!(
            scaffold::to_fetch(ctx, job, &[], &stock),
            Some((key("petramond:dirt"), 64))
        );
        let slot = [Some(stack("petramond:dirt", 64))];
        assert_eq!(scaffold::to_fetch(ctx, job, &slot, &stock), None, "enough in hand");
        job.crew.scaffolding.want = 80;
        assert_eq!(
            scaffold::to_fetch(ctx, job, &slot, &stock),
            Some((key("petramond:dirt"), 16)),
            "a tall pillar asks for its height"
        );
        assert_eq!(scaffold::to_fetch(ctx, job, &[], &BTreeMap::new()), None);
    });
}

#[test]
fn scaffolding_is_kept_in_hand_until_it_is_a_hillsides_worth_of_spoil() {
    let (mut session, id, _) = working(AT);
    let dirt = stack("petramond:dirt", 64);
    at_work(&mut session, id, |ctx, _, job| {
        let one = [Some(dirt.clone())];
        assert!(scaffold::keeps(ctx, job, &one, &dirt));
        let three = [Some(dirt.clone()), Some(dirt.clone()), Some(dirt.clone())];
        assert!(!scaffold::keeps(ctx, job, &three, &dirt));
        let stone = stack("petramond:stone", 5);
        assert!(!scaffold::keeps(ctx, job, &[Some(stone.clone())], &stone));
    });
    session.world.set([4, 0, 4], DIRT);
    session.world.unload([6, 0, 6], [6, 0, 6]);
    let content = &session.builder.content;
    assert_eq!(scaffold::stands(content, [4, 0, 4]), Some(true));
    assert_eq!(scaffold::stands(content, [5, 0, 5]), Some(false));
    assert_eq!(scaffold::stands(content, [6, 0, 6]), None);
}

#[test]
fn a_support_column_rises_from_the_ground_under_work_with_nothing_beside_it() {
    let (mut session, id, _) = working(AT);
    at_work(&mut session, id, |ctx, _, job| {
        let design = &job.design;
        assert!(matches!(
            support::below(ctx, design, [5, 3, 5]),
            Support::Needed([5, 0, 5])
        ));
        assert!(matches!(support::below(ctx, design, [5, 0, 5]), Support::Standing));
        assert!(
            matches!(support::below(ctx, design, [5, 12, 5]), Support::Impossible),
            "too deep"
        );
        assert!(
            matches!(support::below(ctx, design, [1, 3, 0]), Support::Impossible),
            "through the design"
        );
    });
    let open: HashSet<[i32; 3]> = [[5, 3, 5]].into_iter().collect();
    assert!(support::props_up([5, 0, 5], &open));
    assert!(support::props_up([4, 3, 5], &open), "beside it");
    assert!(!support::props_up([7, 0, 5], &open));
}

/// Stone on three sides of `[5, 0, 5]` and over it: the design fills it and
/// the one open side.
fn pocket_site(sealed: bool) -> (Session, crate::project::ProjectId, u64) {
    let (mut session, id) = Session::site(
        "Pocket",
        [6, 1, 6],
        &[([5, 0, 5], "petramond:stone"), ([5, 0, 4], "petramond:stone")],
    );
    for cell in [[4, 0, 5], [6, 0, 5], [5, 0, 6], [5, 1, 5]] {
        session.world.set(cell, STONE);
    }
    if sealed {
        session.world.set([5, 0, 4], STONE);
    }
    session.job(id);
    let golem = session.golem(id, HOME, [5, 0, 2]);
    (session, id, golem)
}

#[test]
fn a_block_that_would_seal_work_in_waits_for_it() {
    let (mut session, id, _) = pocket_site(false);
    let inner = unit(&session, id, [5, 0, 5]);
    let door = unit(&session, id, [5, 0, 4]);
    at_work(&mut session, id, |ctx, _, job| {
        assert_eq!(pocket::seals(ctx, job, door), Some(Some(inner)));
        assert_eq!(pocket::seals(ctx, job, inner), Some(None), "laid first, it seals nothing");
    });
}

#[test]
fn work_sealed_in_has_a_built_neighbour_taken_down_to_reach_it() {
    let (mut session, id, _) = pocket_site(true);
    let inner = unit(&session, id, [5, 0, 5]);
    let door = unit(&session, id, [5, 0, 4]);
    at_work(&mut session, id, |ctx, _, job| {
        assert_eq!(pocket::opener(ctx, job, inner, [5, 0, 2], false), Some(door));
        assert_eq!(pocket::opener(ctx, job, door, [5, 0, 2], false), None, "built");
    });
}

/// A tower of stone seven high with its top still to lay: from the ground
/// no body reaches it.
#[test]
fn a_pillar_is_found_for_work_no_ground_reaches() {
    let tower: Vec<([i32; 3], &str)> = (0..7).map(|y| ([0, y, 0], "petramond:stone")).collect();
    let (mut session, id) = Session::site("Tower", [1, 7, 1], &tower);
    session.world.fill([0, 0, 0], [0, 5, 0], STONE);
    session.world.set(CHEST_AT, AIR);
    session.job(id);
    let golem = session.golem(id, HOME, [3, 0, 1]);
    let top = unit(&session, id, [0, 6, 0]);
    let body = session.body(golem);
    let project = session.builder.projects.get(id).unwrap().clone();
    let found = at_work(&mut session, id, |ctx, _, job| {
        pillar::find(ctx, job, &project, &body, Task::Unit(top), &[[0, 6, 0]], &[[0, 6, 0]])
    });
    let Search::Found(found) = found else {
        panic!("a pillar reaches the top");
    };
    assert!(reaches(feet_of(found.top_cell()), &[[0, 6, 0]]));
    assert_eq!(found.base, 0, "footed on the ground");
    assert!(found.top > found.base);
    assert!(session.world.foothold(found.foot_cell()));
    assert!(session.world.foothold(found.exit));
    let job = &session.builder.jobs.map[&id];
    assert!(!job.design.governed.contains(&found.top_cell()));
}

#[test]
fn the_right_tool_is_taken_up_and_a_trip_is_made_for_one_the_chests_hold() {
    let (mut session, id) = Session::row();
    session.world.set([0, 0, 0], DIRT);
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    session
        .world
        .give(ContainerAddress::Block(CHEST_AT), "petramond:stone_shovel", 1);
    let body = session.body(golem);
    let project = session.builder.projects.get(id).unwrap().clone();
    at_work(&mut session, id, |ctx, _, job| {
        let waiting = cargo::tools_waiting(ctx, job, &body, &project);
        assert_eq!(waiting.into_iter().collect::<Vec<_>>(), vec!["shovel".to_owned()]);
        assert_eq!(cargo::tool_slot(ctx, &body.slots, DIRT), None, "bare hands");
    });
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone_shovel", 1);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        assert!(cargo::tools_waiting(ctx, job, &body, &project).is_empty());
        assert_eq!(cargo::tool_slot(ctx, &body.slots, DIRT), Some(1));
        assert_eq!(cargo::tool_slot(ctx, &body.slots, STONE), None, "a shovel is no pickaxe");
        assert_eq!(cargo::tool_slot(ctx, &body.slots, CHEST), None, "an axe's work");
    });
}

#[test]
fn what_is_carried_covers_what_is_missing_counted_exactly() {
    let carried = cargo::totals(&[
        Some(stack("petramond:stone", 3)),
        None,
        Some(stack("petramond:stone", 2)),
        Some(stack("petramond:dirt", 1)),
    ]);
    assert_eq!(carried.get(&key("petramond:stone")), Some(&5));
    assert!(cargo::holds(&carried, &[stack("petramond:stone", 5)]));
    assert!(!cargo::holds(
        &carried,
        &[stack("petramond:stone", 3), stack("petramond:stone", 3)]
    ));
    assert!(!cargo::holds(&carried, &[stack("petramond:oak_planks", 1)]));
    assert!(cargo::holds(&carried, &[]));
}
