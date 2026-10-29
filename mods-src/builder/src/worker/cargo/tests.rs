use std::collections::{BTreeMap, BTreeSet};

use crate::geometry::feet_of;
use crate::host::fake::rows::{DIRT, STONE};
use crate::host::fake::stack;
use crate::host::prelude::*;
use crate::project::{Hold, Note};
use crate::testing::{Session, CHEST_AT, HOME, TABLE_AT};
use crate::worker::tests::{at_work, working};
use crate::worker::tuning::hands::LID_UP;
use crate::worker::{plan, Step, Then};

use super::load::{fill, Load};
use super::{chest, totals, wants};

#[test]
fn a_full_load_exchanges_later_materials_without_refilling_the_digging_reserve() {
    let cells: Vec<_> = (0..448)
        .map(|x| {
            (
                [x, 0, 0],
                if x < 128 {
                    "petramond:stone"
                } else {
                    "petramond:oak_planks"
                },
            )
        })
        .collect();
    let (mut session, id) = Session::site("Mixed load", [448, 1, 1], &cells);
    session.world.set([0, 0, 0], DIRT);
    session.world.chest(CHEST_AT, 32);
    session.job(id);
    let golem = session.golem(id, HOME, [1, 0, -2]);
    let hands = ContainerAddress::Entity(golem);
    let supply = ContainerAddress::Block(CHEST_AT);
    session
        .world
        .state_mut()
        .containers
        .get_mut(&hands)
        .unwrap()
        .truncate(8);
    session.world.give(hands, "petramond:stone_pickaxe", 1);
    session.world.give(hands, "petramond:dirt", 64);
    for _ in 0..5 {
        session.world.give(hands, "petramond:oak_planks", 64);
    }
    for _ in 0..4 {
        session.world.give(supply, "petramond:stone", 64);
    }
    let before = totals(
        &[
            session.world.container(hands),
            session.world.container(supply),
        ]
        .concat(),
    );
    let since = session.now();
    session.world.set_now(since + LID_UP);
    let body = session.body(golem);
    let predicted = at_work(&mut session, id, |ctx, projects, job| {
        let project = projects.get(id).unwrap();
        match super::preview(ctx, job, &body, project, false) {
            crate::worker::stance::Search::Found(trip) => trip.slots,
            _ => panic!("the exchange must be possible"),
        }
    });
    assert_eq!(
        session.world.container(hands),
        body.slots,
        "pricing a trip cannot move items"
    );
    let reserve = at_work(&mut session, id, |ctx, projects, job| {
        chest::rummage(ctx, projects, job, &body, CHEST_AT, false, since, false);
        wants::dig_room(job)
    });
    let loaded = session.body(golem);
    assert_eq!(
        loaded.slots, predicted,
        "pricing must use the load the chest exchange really produces"
    );
    assert!(
        loaded.slots.iter().filter(|s| s.is_none()).count() >= reserve,
        "fetching must leave room for digging drops: {:?}",
        loaded.slots
    );
    assert!(session.world.count(hands, "petramond:stone") > 0);
    assert_eq!(session.world.count(hands, "petramond:stone_pickaxe"), 1);
    assert!(session.world.count(hands, "petramond:dirt") > 0);
    assert_eq!(
        loaded.slots[0], body.slots[0],
        "the blueprint stays in hand"
    );
    assert_eq!(
        totals(
            &[
                session.world.container(hands),
                session.world.container(supply)
            ]
            .concat()
        ),
        before,
        "exchanging a load must conserve every item"
    );
    let step = at_work(&mut session, id, |ctx, projects, job| {
        plan::plan(ctx, projects, job, &loaded)
    });
    assert!(
        !matches!(
            step,
            Step::Walk {
                then: Then::Deposit(_),
                ..
            }
        ),
        "the next plan must not send the new load straight back: {step:?}"
    );
}

#[test]
fn packing_counts_partial_stacks_and_keeps_item_variants_separate() {
    let (mut session, id, golem) = working([1, 0, -2]);
    let hands = ContainerAddress::Entity(golem);
    let supply = ContainerAddress::Block(CHEST_AT);
    session.world.chest(CHEST_AT, 16);
    session
        .world
        .state_mut()
        .containers
        .get_mut(&hands)
        .unwrap()
        .truncate(8);
    session.world.give(hands, "petramond:stone_pickaxe", 1);
    let variants: Vec<_> = (0..4)
        .map(|n| {
            let mut stack = stack("petramond:stone", 64);
            if n > 0 {
                stack.data = vec![("test:variant".into(), vec![n])];
            }
            stack
        })
        .collect();
    for (slot, stone) in variants.iter().enumerate() {
        session.world.put(supply, slot, Some(stone.clone()));
    }
    for slot in [2, 3] {
        session
            .world
            .put(hands, slot, Some(stack("petramond:stone", 1)));
    }
    let before = totals(
        &[
            session.world.container(hands),
            session.world.container(supply),
        ]
        .concat(),
    );
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        let mut items = BTreeMap::new();
        let room = body.slots.len() - wants::dig_room(job);
        for variant in &variants {
            fill(
                ctx,
                &body.slots,
                &mut items,
                crate::survey::key_of(variant),
                64,
                room,
            );
        }
        let load = Load {
            items,
            tools: BTreeSet::new(),
        };
        for (slot, count) in load.returns(ctx, &body.slots) {
            container_transfer(hands, slot as u32, supply, count);
        }
        let mut missing = load.missing(&container_get(hands).unwrap());
        for (slot, stack) in container_get(supply).unwrap().iter().enumerate() {
            let Some(stack) = stack else { continue };
            let Some(count) = missing.get_mut(&crate::survey::key_of(stack)) else {
                continue;
            };
            if let Some(moved) = container_transfer(
                supply,
                slot as u32,
                hands,
                (*count).min(u32::from(stack.count)) as u8,
            ) {
                *count -= u32::from(moved.count);
            }
        }
        assert!(
            missing.values().all(|n| *n == 0),
            "the planned variants must fit"
        );
        let loaded = container_get(hands).unwrap();
        assert!(loaded.iter().filter(|s| s.is_none()).count() >= wants::dig_room(job));
        assert_eq!(loaded[0], body.slots[0]);
        assert_eq!(loaded[1], body.slots[1]);
    });
    assert_eq!(
        totals(
            &[
                session.world.container(hands),
                session.world.container(supply)
            ]
            .concat()
        ),
        before
    );
}

#[test]
fn a_tool_in_another_chest_keeps_its_place_in_the_load() {
    let (mut session, id, golem) = working([1, 0, -2]);
    let hands = ContainerAddress::Entity(golem);
    let tools_at = [6, 0, -3];
    for x in 2..=6 {
        session.world.chest([x, 0, -3], 4);
    }
    session
        .world
        .give(ContainerAddress::Block(CHEST_AT), "petramond:stone", 3);
    session
        .world
        .give(ContainerAddress::Block(CHEST_AT), "petramond:dirt", 64);
    session.world.give(
        ContainerAddress::Block(tools_at),
        "petramond:stone_shovel",
        1,
    );
    session
        .world
        .state_mut()
        .containers
        .get_mut(&hands)
        .unwrap()
        .truncate(6);
    let since = session.now();
    session.world.set_now(since + LID_UP);
    let body = session.body(golem);
    let step = at_work(&mut session, id, |ctx, projects, job| {
        chest::rummage(ctx, projects, job, &body, CHEST_AT, false, since, false)
    });
    assert!(matches!(step, Step::Walk { then: Then::Fetch(at), .. } if at == tools_at));
    session.world.state_mut().mobs.get_mut(&golem).unwrap().pos = feet_of([6, 0, -2]);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, projects, job| {
        chest::rummage(ctx, projects, job, &body, tools_at, false, since, false);
        let loaded = container_get(hands).unwrap();
        assert!(loaded.iter().filter(|s| s.is_none()).count() >= wants::dig_room(job));
    });
    assert_eq!(session.world.count(hands, "petramond:stone_shovel"), 1);
    assert_eq!(session.world.count(hands, "petramond:stone"), 3);
    assert!(session.world.count(hands, "petramond:dirt") > 0);
}

#[test]
fn an_exchange_waits_when_its_returns_cannot_be_stored() {
    for unread in [false, true] {
        let (mut session, id, golem) = working([1, 0, -2]);
        let hands = ContainerAddress::Entity(golem);
        let supply = ContainerAddress::Block(CHEST_AT);
        session.world.chest(CHEST_AT, 1);
        session.world.give(supply, "petramond:stone", 64);
        session.world.give(hands, "petramond:raw_copper", 64);
        at_work(&mut session, id, |ctx, _, _| {
            ctx.supplies.stock(TABLE_AT);
        });
        if unread {
            session.world.unload(CHEST_AT, CHEST_AT);
        }
        let before = session.world.container(hands);
        let since = session.now();
        session.world.set_now(since + LID_UP);
        let body = session.body(golem);
        at_work(&mut session, id, |ctx, projects, job| {
            chest::rummage(ctx, projects, job, &body, CHEST_AT, false, since, false);
        });
        assert_eq!(session.world.container(hands), before);
        assert_eq!(session.world.count(supply, "petramond:stone"), 64);
        assert_eq!(
            session.builder.projects.get(id).unwrap().hold(),
            if unread { None } else { Some(Hold::Storage) }
        );
    }
}

#[test]
fn scaffolding_cleanup_can_fetch_a_tool_before_handing_everything_in() {
    let (mut session, id) = Session::row();
    session.world.fill([0, 0, 0], [2, 0, 0], STONE);
    session.world.set([4, 0, 0], DIRT);
    session.job(id);
    let golem = session.golem(id, HOME, [1, 0, -2]);
    session.builder.projects.update(id, |p| {
        p.scaffolds.push([4, 0, 0]);
        p.wind_down(Note::None);
    });
    let hands = ContainerAddress::Entity(golem);
    session.world.give(hands, "petramond:dirt", 64);
    session.world.give(
        ContainerAddress::Block(CHEST_AT),
        "petramond:stone_shovel",
        1,
    );
    let since = session.now();
    session.world.set_now(since + LID_UP);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, projects, job| {
        chest::rummage(ctx, projects, job, &body, CHEST_AT, false, since, false);
    });
    assert_eq!(session.world.count(hands, "petramond:stone_shovel"), 1);
    assert!(session.world.count(hands, "petramond:dirt") > 0);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, projects, job| {
        chest::rummage(ctx, projects, job, &body, CHEST_AT, true, since, false);
    });
    assert_eq!(session.world.count(hands, "petramond:stone_shovel"), 0);
    assert_eq!(session.world.count(hands, "petramond:dirt"), 0);
    assert_eq!(session.body(golem).slots[0], body.slots[0]);
}
