use super::*;
use petramond_math::{math::Vec3, world_pos::WorldPos};
use std::collections::BTreeSet;

fn cells(selection: &Selection) -> BTreeSet<[i32; 3]> {
    selection.cells().collect()
}

#[test]
fn extrusion_tracks_each_normal_and_is_one_reversible_edit() {
    for axis in 0..3 {
        for normal in [-1, 1] {
            let mut s = Selection::default();
            s.region([0; 3], [2; 3], false).unwrap();
            let original = cells(&s);
            let face = s
                .surface()
                .faces()
                .iter()
                .find(|f| f.axis == axis && f.normal == normal)
                .unwrap()
                .clone();
            s.begin_extrusion(face);
            for offset in [1, 2, -1, 3] {
                s.extrude(offset).unwrap();
            }
            let mut lo = [0; 3];
            let mut hi = [3; 3];
            if normal < 0 {
                lo[axis] -= 3;
            } else {
                hi[axis] += 3;
            }
            assert_eq!(cells(&s), SelectionBox { lo, hi }.cells().collect());
            let expanded = cells(&s);
            s.finish_extrusion();
            assert!(s.undo());
            assert_eq!(cells(&s), original);
            assert!(s.redo());
            assert_eq!(cells(&s), expanded);
            assert!(s.undo());
            assert!(s.undo());
            assert!(
                s.is_empty(),
                "intermediate drag frames must not enter history"
            );
        }
    }
}

#[test]
fn connected_face_preserves_holes_and_leaves_other_islands_alone() {
    let mut s = Selection::default();
    s.region([0, 0, 0], [4, 1, 4], false).unwrap();
    s.region([1, 0, 1], [3, 1, 3], true).unwrap();
    s.region([5, 0, 5], [6, 1, 6], false).unwrap(); // Corner contact is not an edge.
    let original = cells(&s);
    let surface = s.surface();
    let (face, distance) = surface
        .pick(WorldPos::new(0.5, 8.0, 0.5), -Vec3::Y, 128.0)
        .unwrap();
    assert_eq!(distance, 6.0);
    assert_eq!(
        surface
            .faces()
            .iter()
            .filter(|f| f.axis == 1 && f.normal == 1)
            .count(),
        2
    );
    assert!(surface
        .pick(WorldPos::new(2.5, 8.0, 2.5), -Vec3::Y, 128.0)
        .is_none());
    s.begin_extrusion(face.clone());
    s.extrude(2).unwrap();
    let mut expected = original.clone();
    for p in &original {
        if p[0] < 5 && p[1] == 1 {
            expected.extend([[p[0], 2, p[2]], [p[0], 3, p[2]]]);
        }
    }
    assert_eq!(cells(&s), expected);
    s.extrude(-1).unwrap();
    assert_eq!(
        cells(&s),
        original
            .into_iter()
            .filter(|p| p[0] >= 5 || p[1] == 0)
            .collect()
    );
    s.finish_extrusion();
    assert!(s.undo());
    assert_eq!(s.len(), 40);
}

#[test]
fn surface_picking_excludes_internal_faces_and_works_inside_and_far_away() {
    for origin in [0, 1_000_000_000] {
        let mut s = Selection::default();
        s.region([origin, 0, 0], [origin + 2, 2, 2], false).unwrap();
        s.region([origin + 3, 0, 0], [origin + 3, 0, 0], false)
            .unwrap();
        let surface = s.surface();
        let (face, distance) = surface
            .pick(
                WorldPos::new(f64::from(origin) + 1.5, 0.5, 0.5),
                Vec3::X,
                128.0,
            )
            .unwrap();
        assert_eq!(face.plane, origin + 4);
        assert_eq!(distance, 2.5);
        assert!(surface
            .pick(
                WorldPos::new(f64::from(origin) + 1.5, 0.5, 0.5),
                Vec3::X,
                2.0
            )
            .is_none());
        assert!(surface.pick(WorldPos::ZERO, Vec3::ZERO, 128.0).is_none());
    }
}

#[test]
fn cancelled_and_zero_distance_drags_preserve_the_redo_branch() {
    let mut s = Selection::default();
    s.region([0; 3], [2; 3], false).unwrap();
    s.clear();
    s.undo();
    for cancel in [true, false] {
        let face = s.surface().faces()[0].clone();
        s.begin_extrusion(face);
        s.extrude(7).unwrap();
        if cancel {
            assert!(s.cancel_extrusion());
        } else {
            s.extrude(0).unwrap();
            s.finish_extrusion();
        }
        assert_eq!(s.len(), 27);
        assert!(s.redo());
        assert!(s.is_empty());
        s.undo();
    }
    s.begin_extrusion(s.surface().faces()[0].clone());
    s.extrude(2).unwrap();
    assert!(s.undo(), "undo during a drag cancels it");
    assert_eq!(s.len(), 27);
    assert!(s.redo());
    assert!(s.is_empty());
}

#[test]
fn huge_extrusions_remain_box_geometry_and_rejected_moves_do_not_commit() {
    let mut s = Selection::default();
    s.region([0; 3], [999_999; 3], false).unwrap();
    let face = s
        .surface()
        .faces()
        .iter()
        .find(|f| f.axis == 0 && f.normal == 1)
        .unwrap()
        .clone();
    s.begin_extrusion(face);
    s.extrude(1_000_000).unwrap();
    assert_eq!(s.len(), 2_000_000_000_000_000_000);
    assert_eq!(s.regions().len(), 1);
    assert_eq!(s.outline().len(), 12);
    assert!(s.extrude(i32::MAX).is_err());
    assert_eq!(s.regions().len(), 1);
    assert_eq!(s.extrusion_face().unwrap().plane, 2_000_000);
    s.cancel_extrusion();
    assert_eq!(s.len(), 1_000_000_000_000_000_000);
}

#[test]
fn contraction_follows_uneven_backing_but_never_jumps_across_air_gaps() {
    for normal in [-1, 1] {
        let mut s = Selection::default();
        let mut add = |lo: [i32; 3], hi: [i32; 3]| {
            let a = [lo[0], lo[1] * normal, lo[2]];
            let b = [hi[0], hi[1] * normal, hi[2]];
            s.region(a, b, false).unwrap();
        };
        add([0, 3, 0], [2, 4, 2]);
        add([1, 1, 1], [1, 2, 1]);
        add([0, -3, 0], [2, -2, 2]);
        let original = cells(&s);
        let face = s
            .surface()
            .faces()
            .iter()
            .find(|f| {
                f.axis == 1 && f.normal == normal && f.plane == if normal > 0 { 5 } else { -4 }
            })
            .unwrap()
            .clone();
        s.begin_extrusion(face);
        s.extrude(-3).unwrap();
        assert_eq!(
            cells(&s),
            original
                .iter()
                .copied()
                .filter(|p| p[1] * normal < 2)
                .collect()
        );
        s.extrude(-20).unwrap();
        assert_eq!(
            cells(&s),
            original
                .iter()
                .copied()
                .filter(|p| p[1] * normal < 0)
                .collect()
        );
        s.extrude(0).unwrap();
        assert_eq!(cells(&s), original);
    }
}
