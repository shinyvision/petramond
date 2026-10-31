use super::graph::{Cell, Graph, Work};
use super::grid::Bounds;
use super::recovery;
use crate::worker::Task;

#[test]
fn a_new_block_touching_a_working_ray_at_its_corner_still_blocks_that_ray() {
    let bounds = Bounds::new([0, 0, 0], [3, 3, 3]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let root = bounds.index([0, 1, 0]).unwrap();
    for point in [[3.0, 0.05, 3.0], [2.0, 0.95, 2.0], [2.0, 0.95, 2.0000002]] {
        let mut graph = Graph::new(
            bounds,
            cells.clone(),
            vec![Work {
                unit: 7,
                contacts: vec![root].into(),
            }],
            root,
        );
        graph.set_aim(
            7,
            root,
            super::sight::Aim {
                point,
                potential: false,
                faceless: false,
                misaligned: false,
            },
        );
        assert_eq!(graph.seals(None, &[[2, 0, 1]]), Some(7), "{point:?}");
    }
}

#[test]
fn a_new_placement_face_must_be_visible_from_the_remaining_working_contacts() {
    let bounds = Bounds::new([0, 0, 0], [2, 2, 2]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let root = bounds.index([0, 1, 0]).unwrap();
    let side = bounds.index([1, 0, 1]).unwrap();
    let aim = super::sight::Aim {
        point: [0.5, 0.5, 1.5],
        potential: true,
        faceless: false,
        misaligned: false,
    };
    let work = |contacts: Vec<usize>| {
        vec![Work {
            unit: 7,
            contacts: contacts.into(),
        }]
    };
    let mut graph = Graph::new(bounds, cells.clone(), work(vec![root]), root);
    graph.set_aim(7, root, aim);
    assert_eq!(
        graph.seals(None, &[[0, 0, 0]]),
        Some(7),
        "standing on the new block cannot see its side to place the adjoining block"
    );
    let mut graph = Graph::new(bounds, cells, work(vec![root, side]), root);
    graph.set_aim(7, root, aim);
    graph.set_aim(7, side, aim);
    assert_eq!(
        graph.seals(None, &[[0, 0, 0]]),
        None,
        "a visible alternative face keeps the work accessible"
    );
}

#[test]
fn an_unfaced_placement_is_not_treated_as_visible_from_distant_blocked_stances() {
    use crate::host::fake::rows::STONE;
    use crate::host::prelude::*;
    use crate::testing::{Session, HOME};
    let (mut session, id) = Session::row();
    session.world.fill([2, 0, -7], [2, 7, -5], STONE);
    session.job(id);
    let actor = EntityRef::Entity(session.golem(id, HOME, HOME));
    let bounds = Bounds::new([0, 0, -8], [8, 8, -4]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let mut raw = cells.clone();
    for y in 0..=7 {
        for z in -7..=-5 {
            raw[bounds.index([2, y, z]).unwrap()] = Cell::default();
        }
    }
    let mut cache = super::sight::Cache::default();
    cache.unbudget(0);
    let target = [4, 4, -6];
    let contacts = cache
        .contacts(
            0,
            bounds,
            &cells,
            &raw,
            actor,
            &[target],
            target,
            Some(CellRecord::of("petramond:stone")),
        )
        .unwrap();
    assert!(!contacts.contains(&bounds.index([0, 4, -6]).unwrap()));
    assert!(contacts.contains(&bounds.index([3, 4, -6]).unwrap()));
}

#[test]
fn a_placement_cannot_block_the_only_working_sightline_even_if_the_stance_stays_connected() {
    let bounds = Bounds::new([0, 0, 0], [2, 0, 1]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let root = bounds.index([0, 0, 0]).unwrap();
    let mut graph = Graph::new(
        bounds,
        cells,
        vec![Work {
            unit: 7,
            contacts: vec![root].into(),
        }],
        root,
    );
    graph.set_aim(
        7,
        root,
        super::sight::Aim {
            point: [2.5, 0.5, 0.5],
            potential: false,
            faceless: false,
            misaligned: false,
        },
    );
    assert_eq!(graph.seals(None, &[[1, 0, 1]]), None);
    assert_eq!(graph.seals(None, &[[1, 0, 0]]), Some(7));
}

#[test]
fn losing_the_last_stance_that_sets_a_block_the_right_way_seals_it() {
    let bounds = Bounds::new([0, 0, 0], [2, 0, 1]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let root = bounds.index([0, 0, 0]).unwrap();
    let right = bounds.index([2, 0, 0]).unwrap();
    let wrong = bounds.index([0, 0, 1]).unwrap();
    let mut graph = Graph::new(
        bounds,
        cells,
        vec![Work {
            unit: 7,
            contacts: vec![right, wrong].into(),
        }],
        root,
    );
    for (node, misaligned) in [(right, false), (wrong, true)] {
        let cell = bounds.cell(node);
        graph.set_aim(
            7,
            node,
            super::sight::Aim {
                point: [f64::from(cell[0]) + 0.5, 0.5, f64::from(cell[2]) + 0.5],
                potential: misaligned,
                faceless: false,
                misaligned,
            },
        );
    }
    assert_eq!(
        graph.seals(None, &[[2, 0, 0]]),
        Some(7),
        "a stance that turns the block the wrong way does not keep it buildable"
    );
    assert_eq!(graph.seals(None, &[[0, 0, 1]]), None);
}

#[test]
fn connected_air_does_not_protect_a_working_stance_whose_ground_route_is_cut() {
    let bounds = Bounds::new([0, 0, 0], [3, 0, 1]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let root = bounds.index([0, 0, 0]).unwrap();
    let contact = bounds.index([3, 0, 0]).unwrap();
    let alternate = bounds.index([3, 0, 1]).unwrap();
    let mut graph = Graph::new(
        bounds,
        cells,
        vec![Work {
            unit: 7,
            contacts: vec![contact, alternate].into(),
        }],
        root,
    );
    graph.physical([(7, vec![contact, alternate])].into_iter().collect());
    let wall = [[1, 0, 0]];
    assert_eq!(graph.seals(None, &wall), None);
    let before = |c| c == bounds.cell(alternate);
    assert_eq!(
        graph.seals_ground(None, &wall, &wall, |c| c == bounds.cell(alternate), before),
        None
    );
    assert_eq!(
        graph.seals_ground(None, &wall, &wall, |_| false, before),
        Some(7)
    );
    assert_eq!(
        graph.seals_ground(None, &wall, &wall, |_| false, |_| false),
        None,
        "work the worker already cannot get at is nothing a closure takes away"
    );
}

#[test]
fn sight_survey_is_bounded_across_ticks_and_changes_reask_only_the_stances_they_cross() {
    use crate::host::prelude::*;
    use crate::testing::{Session, HOME};
    let (mut session, id) = Session::row();
    session.job(id);
    let actor = EntityRef::Entity(session.golem(id, HOME, HOME));
    let bounds = Bounds::new([-4, -5, -4], [4, 2, 4]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let mut cache = super::sight::Cache::default();
    let query = |cache: &mut super::sight::Cache, tick| {
        cache.contacts(tick, bounds, &cells, &cells, actor, &[[0; 3]], [0; 3], None)
    };
    cache.budget(0, 1);
    assert!(query(&mut cache, 0).is_none());
    assert!(
        query(&mut cache, 0).is_none(),
        "another call cannot spend the next tick's budget"
    );
    let mut finished = false;
    for tick in 1..bounds.len() as u64 + 1 {
        if query(&mut cache, tick).is_some() {
            finished = true;
            break;
        }
    }
    assert!(finished);
    cache.changed(&[[20; 3]], false);
    assert!(query(&mut cache, 10).is_some());
    cache.changed(&[[4, -5, 4]], false);
    cache.budget(11, 0);
    assert!(
        query(&mut cache, 11).is_some(),
        "a nearby change crossing no stance's footing or sightline keeps every answer"
    );
    cache.changed(&[[2, 0, 0]], false);
    cache.budget(12, 0);
    assert!(
        query(&mut cache, 12).is_none(),
        "a change across a sightline asks that stance again"
    );
    cache.changed(&[[0; 3]], false);
    cache.budget(13, 1);
    assert!(
        query(&mut cache, 13).is_none(),
        "changed geometry must be surveyed again"
    );
}

#[test]
fn a_distant_room_is_protected_and_multiple_entrances_are_checked_together() {
    let bounds = Bounds::new([0, 0, 0], [40, 0, 2]);
    let mut cells = vec![Cell::default(); bounds.len()];
    for x in 0..=40 {
        for z in [0, 2] {
            cells[bounds.index([x, 0, z]).unwrap()].open = true;
        }
    }
    for x in [0, 40] {
        cells[bounds.index([x, 0, 1]).unwrap()].open = true;
    }
    let root = bounds.index([0, 0, 1]).unwrap();
    let far = bounds.index([40, 0, 1]).unwrap();
    let mut graph = Graph::new(
        bounds,
        cells,
        vec![Work {
            unit: 7,
            contacts: vec![far].into(),
        }],
        root,
    );
    assert!(graph.sealed.is_empty());
    assert_eq!(
        graph.seals(None, &[[1, 0, 0]]),
        None,
        "the other entrance remains"
    );
    assert_eq!(graph.seals(None, &[[1, 0, 0], [1, 0, 2]]), Some(7));
    assert_eq!(
        graph.seals(Some(7), &[[1, 0, 0], [1, 0, 2]]),
        None,
        "completed work needs no access"
    );
    assert_eq!(
        graph.seals(None, &[[40, 0, 1]]),
        Some(7),
        "the last working face matters even without a cut vertex"
    );
}

#[test]
fn recovery_plans_all_regions_and_prefers_an_opening_shared_by_more_work() {
    let bounds = Bounds::new([0, 0, 0], [4, 0, 2]);
    let root = bounds.index([0, 0, 1]).unwrap();
    let shared = bounds.index([1, 0, 0]).unwrap();
    let other = bounds.index([1, 0, 2]).unwrap();
    let mut cells = vec![Cell::default(); bounds.len()];
    for cell in [
        [0, 0, 0],
        [0, 0, 1],
        [0, 0, 2],
        [2, 0, 0],
        [3, 0, 0],
        [4, 0, 0],
        [2, 0, 2],
    ] {
        cells[bounds.index(cell).unwrap()].open = true;
    }
    cells[shared].dig = Some((Task::Reopen(10), 1));
    cells[other].dig = Some((Task::Reopen(11), 1));
    let work = || {
        vec![
            Work {
                unit: 0,
                contacts: vec![bounds.index([3, 0, 0]).unwrap()].into(),
            },
            Work {
                unit: 1,
                contacts: vec![bounds.index([4, 0, 0]).unwrap()].into(),
            },
            Work {
                unit: 2,
                contacts: vec![bounds.index([2, 0, 2]).unwrap()].into(),
            },
        ]
    };
    let graph = Graph::new(bounds, cells.clone(), work(), root);
    assert_eq!(graph.sealed.len(), 3);
    let (gates, served) = recovery::plan(&graph);
    assert_eq!(served.len(), 3, "every sealed unit has an opening");
    assert_eq!(
        gates.iter().map(|g| g.task).collect::<Vec<_>>(),
        vec![Task::Reopen(10), Task::Reopen(11)]
    );
    assert_eq!(gates[0].serves, 2);

    cells[shared].open = true;
    let mut graph = Graph::new(bounds, cells.clone(), work(), root);
    assert_eq!(graph.sealed.len(), 1);
    assert_eq!(
        graph.seals(None, &[[1, 0, 0]]),
        Some(0),
        "already exposed work cannot be resealed while another region is opened"
    );
    cells[other].open = true;
    let graph = Graph::new(bounds, cells, work(), root);
    assert!(graph.sealed.is_empty());
    assert!(recovery::plan(&graph).0.is_empty());
}

#[test]
fn sealed_work_no_opening_reaches_is_not_served() {
    let bounds = Bounds::new([0, 0, 0], [4, 0, 0]);
    let mut cells = vec![Cell::default(); bounds.len()];
    for x in [0, 1, 4] {
        cells[bounds.index([x, 0, 0]).unwrap()].open = true;
    }
    cells[bounds.index([2, 0, 0]).unwrap()].dig = Some((Task::Breakout([2, 0, 0]), 1));
    let root = bounds.index([0, 0, 0]).unwrap();
    let work = vec![Work {
        unit: 5,
        contacts: vec![bounds.index([4, 0, 0]).unwrap()].into(),
    }];
    let graph = Graph::new(bounds, cells, work, root);
    assert_eq!(graph.sealed.len(), 1);
    let (gates, served) = recovery::plan(&graph);
    assert!(
        gates.is_empty() && served.is_empty(),
        "a block nothing may dig stands in the way"
    );
}

#[test]
fn cells_closed_after_the_graph_was_built_count_toward_every_seal_check() {
    let bounds = Bounds::new([0, 0, 0], [40, 0, 2]);
    let mut cells = vec![Cell::default(); bounds.len()];
    for x in 0..=40 {
        for z in [0, 2] {
            cells[bounds.index([x, 0, z]).unwrap()].open = true;
        }
    }
    for x in [0, 40] {
        cells[bounds.index([x, 0, 1]).unwrap()].open = true;
    }
    let root = bounds.index([0, 0, 1]).unwrap();
    let far = bounds.index([40, 0, 1]).unwrap();
    let mut graph = Graph::new(
        bounds,
        cells,
        vec![Work {
            unit: 7,
            contacts: vec![far].into(),
        }],
        root,
    );
    assert_eq!(graph.seals(None, &[[1, 0, 0]]), None);
    graph.since(&[[1, 0, 2]], vec![[1, 0, 2]], Default::default());
    assert_eq!(
        graph.seals(None, &[[1, 0, 0]]),
        Some(7),
        "the other entrance closed after the graph was built"
    );
    graph.since(&[[1, 0, 2]], vec![[1, 0, 2]], [7].into_iter().collect());
    assert_eq!(
        graph.seals(None, &[[1, 0, 0]]),
        None,
        "work finished since needs no access"
    );
}

#[test]
fn patching_changed_cells_classifies_the_site_as_a_full_pass_would() {
    use crate::host::fake::rows::{AIR, DIRT, STONE, TORCH};
    use crate::testing::{Session, HOME};
    use crate::worker::tests::{at_work, unit};
    let (mut session, id) = Session::row();
    session.job(id);
    session.golem(id, HOME, [1, 0, -2]);
    let placed = unit(&session, id, [1, 0, 0]);
    let world = std::rc::Rc::clone(&session.world);
    at_work(&mut session, id, |ctx, projects, job| {
        let project = projects.get(id).unwrap().clone();
        let changed = [[1, 0, 0], [3, -1, 1], [3, 0, 1], [-3, 0, 1]];
        world.set(changed[0], STONE);
        world.set(changed[1], AIR);
        world.set(changed[2], DIRT);
        world.set(changed[3], TORCH);
        job.access_plan.changed(&changed, false);
        if let Some(survey) = &mut job.survey {
            survey.measure(&job.design, &[placed]);
        }
        job.access_plan.unbudget_sight(ctx.now);
        assert!(super::refresh(ctx, job, &project));
        let plan = &job.access_plan;
        let mut whole = super::Classifier::new(
            ctx,
            &job.design,
            job.survey.as_ref().unwrap(),
            plan.snapshot.as_ref().unwrap(),
            &project,
        );
        let (cells, raw) = whole.all(ctx);
        let prepared = plan.prepared.as_ref().unwrap();
        assert!(prepared.raw == raw, "raw cells match a full pass");
        assert!(prepared.cells == cells, "body cells match a full pass");
        assert!(
            plan.removable.contains(&[1, 0, 0]),
            "a satisfied unit can be reopened"
        );
    });
}

#[test]
fn local_cut_search_finds_exactly_the_cells_a_full_flood_loses() {
    let bounds = Bounds::new([0, 0, 0], [7, 3, 7]);
    let mut seed = 0x2545_f491_u64;
    let mut next = move |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % n
    };
    let mut compared = 0;
    for _ in 0..300 {
        let mut cells = vec![Cell::default(); bounds.len()];
        for cell in &mut cells {
            cell.open = next(10) < 7;
        }
        let root = next(bounds.len() as u64) as usize;
        cells[root].open = true;
        let graph = Graph::new(bounds, cells, Vec::new(), root);
        let reached: Vec<usize> = (0..bounds.len()).filter(|i| graph.reached[*i]).collect();
        for _ in 0..10 {
            let mut blocked: Vec<usize> = (0..1 + next(4))
                .map(|_| reached[next(reached.len() as u64) as usize])
                .collect();
            blocked.sort_unstable();
            blocked.dedup();
            let full = graph.flood_without(&graph.reached, &blocked);
            let Some(lost) = graph.detached(&graph.reached, &blocked) else {
                let cut = reached
                    .iter()
                    .any(|i| !full[*i] && blocked.binary_search(i).is_err());
                assert!(blocked.contains(&root) || cut, "blocked {blocked:?}");
                continue;
            };
            let expected: crate::fx::HashSet<usize> = reached
                .iter()
                .copied()
                .filter(|i| !full[*i] && blocked.binary_search(i).is_err())
                .collect();
            assert_eq!(lost, expected, "blocked {blocked:?}");
            compared += 1;
        }
    }
    assert!(compared > 1000);
}

#[test]
fn neighbour_indices_match_the_cells_one_face_away() {
    let bounds = Bounds::new([-3, 2, 5], [1, 5, 11]);
    for i in 0..bounds.len() {
        let cell = bounds.cell(i);
        let expected =
            crate::geometry::FACES.map(|f| bounds.index(std::array::from_fn(|a| cell[a] + f[a])));
        assert_eq!(bounds.neighbours(i), expected, "{cell:?}");
    }
}

#[test]
fn checking_only_the_work_a_change_touches_answers_as_a_full_scan_would() {
    let bounds = Bounds::new([0, 0, 0], [9, 0, 2]);
    let mut cells = vec![Cell::default(); bounds.len()];
    for x in 0..=9 {
        for z in 0..=1 {
            cells[bounds.index([x, 0, z]).unwrap()].open = true;
        }
    }
    let pocket = bounds.index([5, 0, 2]).unwrap();
    cells[pocket].open = true;
    let root = bounds.index([0, 0, 0]).unwrap();
    let work = vec![Work {
        unit: 7,
        contacts: vec![pocket].into(),
    }];
    let mut graph = Graph::new(bounds, cells, work, root);
    assert_eq!(
        graph.seals(None, &[[5, 0, 1]]),
        Some(7),
        "work standing only in a pocket the change cuts off"
    );
    let bounds = Bounds::new([0, 0, 0], [47, 0, 47]);
    let mut seed = 0x51ed_u64;
    let mut next = move |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % n
    };
    let mut sealed = 0;
    for _ in 0..200 {
        let mut cells = vec![Cell::default(); bounds.len()];
        for cell in &mut cells {
            cell.open = next(20) < 13;
        }
        let root = next(bounds.len() as u64) as usize;
        cells[root].open = true;
        let mut work = Vec::new();
        let mut aims = Vec::new();
        for unit in 0..60 {
            let target = bounds.cell(next(bounds.len() as u64) as usize);
            let contacts: Vec<usize> = (0..1 + next(4))
                .filter_map(|_| {
                    let c: [i32; 3] = std::array::from_fn(|a| target[a] + next(7) as i32 - 3);
                    let c = [c[0], 0, c[2]];
                    bounds.index(c).filter(|i| cells[*i].open)
                })
                .collect();
            let aimed = next(2) == 0;
            for c in &contacts {
                if aimed {
                    let point = target.map(|v| f64::from(v) + 0.5);
                    aims.push((
                        unit,
                        *c,
                        super::sight::Aim {
                            point,
                            potential: next(2) == 0,
                            faceless: false,
                            misaligned: false,
                        },
                    ));
                }
            }
            work.push(Work {
                unit,
                contacts: contacts.into(),
            });
        }
        let mut graph = Graph::new(bounds, cells, work, root);
        for (unit, c, aim) in aims {
            graph.set_aim(unit, c, aim);
        }
        for _ in 0..10 {
            let blocked: Vec<[i32; 3]> = (0..1 + next(3))
                .map(|_| bounds.cell(next(bounds.len() as u64) as usize))
                .collect();
            graph.scan_all = false;
            let quick = graph.seals(None, &blocked);
            graph.scan_all = true;
            let full = graph.seals(Some(usize::MAX), &blocked);
            assert_eq!(quick, full, "blocked {blocked:?}");
            sealed += usize::from(full.is_some());
            let closed: Vec<[i32; 3]> = (0..1 + next(3))
                .map(|_| bounds.cell(next(bounds.len() as u64) as usize))
                .collect();
            graph.since(&closed, closed.clone(), Default::default());
            for candidate in [Vec::new(), blocked.clone()] {
                graph.scan_all = true;
                if !candidate.is_empty() && graph.seals(Some(usize::MAX), &[]).is_some() {
                    // An out-of-date graph is only consulted once the closures alone seal nothing.
                    continue;
                }
                graph.scan_all = false;
                let quick = graph.seals(None, &candidate);
                graph.scan_all = true;
                let full = graph.seals(Some(usize::MAX), &candidate);
                assert_eq!(quick, full, "closed {closed:?} candidate {candidate:?}");
                sealed += usize::from(full.is_some());
            }
            graph.since(&[], Vec::new(), Default::default());
        }
    }
    assert!(sealed > 100);
}

#[test]
fn work_with_nothing_to_be_placed_against_cannot_veto_a_placement() {
    let bounds = Bounds::new([0, 0, 0], [2, 2, 2]);
    let cells = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let root = bounds.index([0, 1, 0]).unwrap();
    let work = || {
        vec![Work {
            unit: 7,
            contacts: vec![root].into(),
        }]
    };
    let aim = super::sight::Aim {
        point: [0.5, 0.5, 1.5],
        potential: true,
        faceless: false,
        misaligned: false,
    };
    let mut graph = Graph::new(bounds, cells.clone(), work(), root);
    graph.set_aim(7, root, aim);
    assert_eq!(graph.seals(None, &[[0, 0, 0]]), Some(7));
    let mut graph = Graph::new(bounds, cells, work(), root);
    graph.set_aim(
        7,
        root,
        super::sight::Aim {
            faceless: true,
            misaligned: false,
            ..aim
        },
    );
    assert_eq!(
        graph.seals(None, &[[0, 0, 0]]),
        None,
        "it cannot be placed either way, and the placement may give it its face"
    );
}
