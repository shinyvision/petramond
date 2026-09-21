use super::*;

#[test]
fn cuboid_outline_has_only_twelve_boundary_lines() {
    let mut s = Selection::default();
    s.region([-12, -5, 2], [15, 9, 30], false).unwrap();
    let edges = s.outline();
    assert_eq!(edges.len(), 12);
    for [a, b] in edges {
        for axis in 0..3 {
            if a[axis] == b[axis] {
                assert!([[-12, 16], [-5, 10], [2, 31]][axis].contains(&a[axis]));
            }
        }
    }
}

#[test]
fn attached_cell_extends_the_union_without_coplanar_seams() {
    let mut s = Selection::default();
    s.region([0, 0, 0], [1, 0, 0], false).unwrap();
    s.region([0, 0, 1], [0, 0, 1], false).unwrap();
    let edges = s.outline();
    assert_eq!(
        edges.len(),
        18,
        "six-sided L footprint, top and bottom plus six uprights"
    );
    assert!(
        !edges.contains(&[[0, 1, 1], [1, 1, 1]]),
        "attached cell shares the top surface"
    );
    assert!(
        !edges.contains(&[[1, 1, 0], [1, 1, 1]]),
        "original cuboid has no grid seam"
    );
    s.region([0, 0, 0], [0, 0, 0], true).unwrap();
    assert!(s.undo());
    assert_eq!(s.outline(), edges);
}

#[test]
fn boundary_geometry_matches_exposed_voxel_edges_after_mixed_edits() {
    use std::collections::{BTreeMap, BTreeSet};
    let mut selection = Selection::default();
    let mut seed = 194u32;
    for edit in 0..40 {
        let mut coordinate = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed % 7) as i32 - 3
        };
        let a = std::array::from_fn(|_| coordinate());
        let b = std::array::from_fn(|_| coordinate());
        selection.region(a, b, edit % 3 == 0).unwrap();
        let cells: BTreeSet<_> = selection.cells().collect();
        let mut faces = BTreeMap::<([i32; 3], [i32; 3]), u8>::new();
        for p in &cells {
            for axis in 0..3 {
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                for side in 0..2 {
                    let mut neighbor = *p;
                    neighbor[axis] += if side == 0 { -1 } else { 1 };
                    if cells.contains(&neighbor) {
                        continue;
                    }
                    let mut corners = [*p; 4];
                    for c in &mut corners {
                        c[axis] += side;
                    }
                    corners[1][u] += 1;
                    corners[2][u] += 1;
                    corners[2][v] += 1;
                    corners[3][v] += 1;
                    for i in 0..4 {
                        let a = corners[i];
                        let b = corners[(i + 1) % 4];
                        *faces.entry((a.min(b), a.max(b))).or_default() ^=
                            1 << (axis * 2 + side as usize);
                    }
                }
            }
        }
        let expected: BTreeSet<_> = faces
            .into_iter()
            .filter_map(|(edge, mask)| (mask != 0).then_some(edge))
            .collect();
        let mut actual = BTreeSet::new();
        for [a, b] in selection.outline() {
            let axis = (0..3).find(|i| a[*i] != b[*i]).unwrap();
            for coordinate in a[axis]..b[axis] {
                let mut lo = a;
                lo[axis] = coordinate;
                let mut hi = lo;
                hi[axis] += 1;
                assert!(actual.insert((lo, hi)), "no duplicate boundary segments");
            }
        }
        assert_eq!(actual, expected, "edit {edit}");
    }
}
