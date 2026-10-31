use super::*;
use crate::worker::pocket::grid::Bounds;

#[test]
fn sealed_work_below_a_floor_opens_a_safe_shared_sightline_from_reachable_ground() {
    let bounds = Bounds::new([0, 0, 0], [4, 5, 4]);
    let mut raw = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let roof = [2, 2, 2];
    raw[bounds.index(roof).unwrap()] = Cell {
        dig: Some((Task::Reopen(9), 2)),
        ..Cell::default()
    };
    let root = bounds.index([2, 3, 2]).unwrap();
    let graph = Graph::new(bounds, raw.clone(), Vec::new(), root);
    let targets = [(0, [2, 0, 2]), (1, [2, 1, 2])];
    let gates = Network::new(&graph, &raw, &targets, &[[2, 3, 2]]).openings(&[0, 1]);
    assert_eq!(gates.len(), 1);
    assert_eq!(gates[0].task, Task::Reopen(9));
    assert_eq!(gates[0].serves, 2);
    raw[bounds.index(roof).unwrap()].dig = None;
    assert!(
        Network::new(&graph, &raw, &targets, &[[2, 3, 2]])
            .openings(&[0, 1])
            .is_empty(),
        "protected blocks are never proposed as openings"
    );
}

#[test]
fn an_elevated_side_wall_does_not_turn_future_scaffold_work_into_work_below_a_floor() {
    let bounds = Bounds::new([0, 0, 0], [4, 4, 4]);
    let mut raw = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let wall = [2, 1, 2];
    raw[bounds.index(wall).unwrap()] = Cell {
        dig: Some((Task::Reopen(9), 2)),
        ..Cell::default()
    };
    let root = bounds.index([2, 2, 0]).unwrap();
    let graph = Graph::new(bounds, raw.clone(), Vec::new(), root);
    let network = Network::new(&graph, &raw, &[(0, [2, 1, 3])], &[[2, 2, 0]]);
    assert!(!network.openings(&[0]).is_empty());
    assert!(network.below().next().is_none());
}
