use super::*;

fn contains(mask: &Mask, cell: [i32; 3]) -> bool {
    let mut node = 0;
    while node < mask.bounds[0][3] as usize {
        let lo = mask.cells[node * 2];
        let hi = mask.cells[node * 2 + 1];
        if (0..3).all(|i| lo[i] <= cell[i] && cell[i] < hi[i]) {
            if lo[3] != 0 {
                return true;
            }
            node += 1;
        } else {
            node = hi[3] as usize;
        }
    }
    false
}

#[test]
fn sparse_mask_preserves_holes_islands_and_undo() {
    let mut selection = Selection::default();
    selection.region([-3, -2, -3], [3, 2, 3], false).unwrap();
    selection.region([-1; 3], [1; 3], true).unwrap();
    let distant = [25_000_003, -301, -25_000_001];
    selection.region(distant, distant, false).unwrap();
    let mask = Mask::new(&selection);
    for x in -4..=4 {
        for y in -3..=3 {
            for z in -4..=4 {
                let cell = [x, y, z];
                assert_eq!(contains(&mask, cell), selection.contains(cell));
            }
        }
    }
    assert!(contains(&mask, distant));
    assert!(!contains(&mask, [distant[0] + 1, distant[1], distant[2]]));
    assert!(selection.undo());
    assert!(!contains(&Mask::new(&selection), distant));
    assert!(selection.undo());
    assert!(contains(&Mask::new(&selection), [0; 3]));
    selection.clear();
    assert!(!contains(&Mask::new(&selection), [0; 3]));
}

#[test]
fn large_selection_upload_depends_on_geometry_not_volume() {
    let mut selection = Selection::default();
    selection.region([-10_000; 3], [10_000; 3], false).unwrap();
    let mask = Mask::new(&selection);
    assert_eq!(mask.cells.len(), 2, "one leaf regardless of box volume");
    for p in [[0; 3], [-10_000; 3], [10_000; 3]] {
        assert!(contains(&mask, p));
    }
    assert!(!contains(&mask, [10_001; 3]));
}
