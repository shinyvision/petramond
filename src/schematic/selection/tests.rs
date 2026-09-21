use super::*;
use petramond_math::{math::Vec3, world_pos::WorldPos};

#[test]
fn overlapping_edits_undo_to_the_exact_previous_union() {
    let mut s = Selection::default();
    s.region([0, 0, 0], [2, 2, 2], false).unwrap();
    let original: Vec<_> = s.cells().collect();
    s.region([1, 1, 1], [3, 3, 3], false).unwrap();
    let union: Vec<_> = s.cells().collect();
    s.region([0, 1, 0], [3, 1, 3], true).unwrap();
    s.region([20, 20, 20], [21, 21, 21], true).unwrap();
    assert!(s.undo(), "empty remove does not consume an undo step");
    assert_eq!(s.cells().collect::<Vec<_>>(), union);
    assert!(s.undo());
    assert_eq!(s.cells().collect::<Vec<_>>(), original);
    s.clear();
    assert!(s.undo());
    assert_eq!(s.cells().collect::<Vec<_>>(), original);
}

#[test]
fn redo_replays_overlaps_subtraction_and_clear_in_order() {
    let mut s = Selection::default();
    let mut states = vec![Vec::new()];
    s.region([0, 0, 0], [2, 2, 2], false).unwrap();
    states.push(s.cells().collect::<Vec<_>>());
    s.region([1, 1, 1], [3, 3, 3], false).unwrap();
    states.push(s.cells().collect());
    s.region([0, 1, 0], [3, 1, 3], true).unwrap();
    states.push(s.cells().collect());
    s.clear();
    states.push(Vec::new());
    for _ in 0..2 {
        for expected in states[..states.len() - 1].iter().rev() {
            let revision = s.revision();
            assert!(s.undo());
            assert_ne!(s.revision(), revision);
            assert_eq!(&s.cells().collect::<Vec<_>>(), expected);
        }
        assert!(!s.undo());
        for expected in &states[1..] {
            let revision = s.revision();
            assert!(s.redo());
            assert_ne!(s.revision(), revision);
            assert_eq!(&s.cells().collect::<Vec<_>>(), expected);
        }
        assert!(!s.redo());
    }
}

#[test]
fn new_selection_edits_clear_redo_but_noops_and_rejected_edits_preserve_it() {
    let mut s = Selection::default();
    s.region([0; 3], [1; 3], false).unwrap();
    s.region([2; 3], [2; 3], false).unwrap();
    assert!(s.undo());
    s.region([0; 3], [1; 3], false).unwrap();
    s.region([10; 3], [10; 3], true).unwrap();
    assert!(s.region([0; 3], [i32::MAX; 3], false).is_err());
    assert!(s.redo());
    assert!(s.undo());
    s.region([3; 3], [3; 3], false).unwrap();
    assert!(!s.redo());
    assert!(s.undo());
    s.clear();
    assert!(!s.redo());
    assert!(s.undo());
    assert!(s.undo());
    s.clear();
    assert!(s.redo(), "clearing an empty selection preserves redo");
}

#[test]
fn selection_history_stays_bounded_across_undo_and_redo() {
    let mut s = Selection::default();
    for x in 0..100 {
        s.region([x, 0, 0], [x, 0, 0], false).unwrap();
    }
    let retained = s.undo.len();
    assert!(retained < 100);
    for _ in 0..3 {
        let mut count = 0;
        while s.undo() {
            count += 1;
        }
        assert_eq!(count, retained);
        assert_eq!(s.len(), (100 - retained) as u64);
        let mut count = 0;
        while s.redo() {
            count += 1;
        }
        assert_eq!(count, retained);
        assert_eq!(s.len(), 100);
    }
}

#[test]
fn selection_raycast_hits_air_cells_with_axis_aligned_and_negative_rays() {
    let mut s = Selection::default();
    s.region([-2, 0, 5], [0, 1, 7], false).unwrap();
    let eye = WorldPos::new(-0.5, 0.5, -10.0);
    assert_eq!(s.raycast(eye, Vec3::Z, 128.0), Some([-1, 0, 5]));
    assert_eq!(s.raycast(eye, Vec3::Z, 14.0), None);
    assert_eq!(
        s.raycast(WorldPos::new(-0.5, 0.5, 10.0), -Vec3::Z, 128.0),
        Some([-1, 0, 7])
    );
    let p = s.raycast(eye, Vec3::Z, 128.0).unwrap();
    s.region(p, p, true).unwrap();
    assert_eq!(s.raycast(eye, Vec3::Z, 128.0), Some([-1, 0, 6]));
    assert!(s.undo());
    assert_eq!(s.raycast(eye, Vec3::Z, 128.0), Some(p));
}

#[test]
fn massive_regions_and_holes_store_only_geometry() {
    let mut s = Selection::default();
    s.region([-10_000; 3], [9_999; 3], false).unwrap();
    assert_eq!(s.len(), 8_000_000_000_000);
    assert_eq!(s.regions().len(), 1);
    assert_eq!(s.outline().len(), 12);
    s.region([-100; 3], [99; 3], true).unwrap();
    assert_eq!(s.len(), 8_000_000_000_000 - 8_000_000);
    assert_eq!(s.regions().len(), 6);
    assert_eq!(s.outline().len(), 24);
    assert!(!s.contains([0; 3]));
    assert!(s.undo());
    assert_eq!(s.regions().len(), 1);
    assert!(s.redo());
    assert!(!s.contains([0; 3]));
}

#[test]
fn geometry_matches_voxel_union_under_mixed_edits() {
    let mut s = Selection::default();
    let mut expected = std::collections::BTreeSet::new();
    let mut seed = 123u32;
    for step in 0..160 {
        let mut coordinate = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed % 9) as i32 - 4
        };
        let a = std::array::from_fn(|_| coordinate());
        let b = std::array::from_fn(|_| coordinate());
        let remove = step % 3 == 0;
        s.region(a, b, remove).unwrap();
        for p in SelectionBox::between(a, b).unwrap().cells() {
            if remove {
                expected.remove(&p);
            } else {
                expected.insert(p);
            }
        }
        assert_eq!(s.len(), expected.len() as u64);
        assert_eq!(
            s.cells().collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        for (i, r) in s.regions().iter().enumerate() {
            assert!(s.regions()[i + 1..]
                .iter()
                .all(|other| r.intersection(*other).is_none()));
        }
    }
}
