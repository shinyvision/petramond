use super::*;
use crate::worker::pocket::{graph::Work, grid::Bounds};

#[test]
fn future_footholds_need_a_clear_column_from_reachable_dry_ground() {
    let bounds = Bounds::new([0, 0, 0], [3, 5, 0]);
    let mut raw = vec![
        Cell {
            open: true,
            ..Cell::default()
        };
        bounds.len()
    ];
    let base = bounds.index([0, 0, 0]).unwrap();
    let top = bounds.index([0, 4, 0]).unwrap();
    let mut graph = Graph::new(
        bounds,
        raw.clone(),
        vec![Work {
            unit: 7,
            contacts: vec![top].into(),
        }],
        base,
    );
    graph.ground.insert(base);
    graph.supports = columns(&graph, &raw, &HashSet::default());
    graph.physical([(7, vec![top])].into_iter().collect());
    assert!(graph.sealed.is_empty());
    let before = |c| c == [0, 0, 0];
    assert_eq!(graph.seals_ground(None, &[], &[], before, before), None);
    assert_eq!(
        graph.seals_ground(None, &[], &[], |_| false, before),
        Some(7)
    );
    assert_eq!(
        graph.seals_ground(None, &[], &[[0, 2, 0]], before, before),
        Some(7)
    );
    assert!(
        !graph
            .supports
            .contains_key(&bounds.index([1, 4, 0]).unwrap()),
        "air without a reachable base cannot support a scaffold"
    );
    let governed = [[0, 2, 0]].into_iter().collect();
    assert!(!columns(&graph, &raw, &governed).contains_key(&top));
    raw[bounds.index([0, 1, 0]).unwrap()].open = false;
    assert!(!columns(&graph, &raw, &HashSet::default()).contains_key(&top));
}
