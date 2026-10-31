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
                contacts: vec![root],
            }],
            root,
        );
        graph.set_aim(
            7,
            root,
            super::sight::Aim {
                point,
                potential: false,
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
    };
    let work = |contacts| vec![Work { unit: 7, contacts }];
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
            contacts: vec![root],
        }],
        root,
    );
    graph.set_aim(
        7,
        root,
        super::sight::Aim {
            point: [2.5, 0.5, 0.5],
            potential: false,
        },
    );
    assert_eq!(graph.seals(None, &[[1, 0, 1]]), None);
    assert_eq!(graph.seals(None, &[[1, 0, 0]]), Some(7));
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
            contacts: vec![contact, alternate],
        }],
        root,
    );
    graph.physical([(7, vec![contact, alternate])].into_iter().collect());
    let wall = [[1, 0, 0]];
    assert_eq!(graph.seals(None, &wall), None);
    assert_eq!(
        graph.seals_ground(None, &wall, &wall, |c| c == bounds.cell(alternate)),
        None
    );
    assert_eq!(graph.seals_ground(None, &wall, &wall, |_| false), Some(7));
}

#[test]
fn sight_survey_is_bounded_across_ticks_and_nearby_changes_invalidate_its_answers() {
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
    cache.changed(&[[0; 3]], false);
    cache.budget(11, 1);
    assert!(
        query(&mut cache, 11).is_none(),
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
            contacts: vec![far],
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
                contacts: vec![bounds.index([3, 0, 0]).unwrap()],
            },
            Work {
                unit: 1,
                contacts: vec![bounds.index([4, 0, 0]).unwrap()],
            },
            Work {
                unit: 2,
                contacts: vec![bounds.index([2, 0, 2]).unwrap()],
            },
        ]
    };
    let graph = Graph::new(bounds, cells.clone(), work(), root);
    assert_eq!(graph.sealed.len(), 3);
    let gates = recovery::plan(&graph);
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
    assert!(recovery::plan(&graph).is_empty());
}
