use super::*;

#[test]
fn spatial_pruning_visits_every_intersecting_cut() {
    let mut cuts = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            cuts.extend(plan(7, [x, z]));
        }
    }
    let index = Index::build(&mut cuts);
    let mut rng = FeatureRng::from_state(93);
    for _ in 0..512 {
        let p = [
            rng.next_i32(-100, 150),
            rng.next_i32(-60, 100),
            rng.next_i32(-100, 150),
        ];
        let bounds = [p.map(|v| v - 4), p.map(|v| v + 4)];
        let mut collected = 0;
        assert!(index
            .visit(&cuts, bounds, |_| {
                collected += 1;
                ControlFlow::Continue(())
            })
            .is_continue());
        assert_eq!(
            collected,
            cuts.iter().filter(|cut| cut.intersects(bounds)).count()
        );
    }
}

#[test]
fn walks_are_bounded_and_independent_of_the_gather_window() {
    for seed in [7, 19] {
        for cell in [[0, 0], [-1, 2], [3, -4]] {
            for cut in plan(seed, cell) {
                for axis in [0, 2] {
                    let start = cell[axis / 2] * CELL;
                    assert!(cut.center[axis] - cut.radius >= (start - REACH) as f64);
                    assert!(cut.center[axis] + cut.radius <= (start + CELL + REACH) as f64);
                }
            }
        }
    }
    let center = plan(7, [0, 0])[0].center.map(|v| v.round() as i32);
    let memos = WalkMemos::new(crate::cache::CacheBudget::REFERENCE);
    let context = crate::cache::GenContext::installed(7);
    let large = WalkField::gather(
        &memos,
        context,
        [center.map(|v| v - 80), center.map(|v| v + 80)],
    );
    let small = WalkField::gather(
        &memos,
        context,
        [center.map(|v| v - 8), center.map(|v| v + 8)],
    );
    let at = |cuts: &[Cut], p: [f64; 3]| cuts.iter().fold(1.0_f64, |v, c| v.min(c.density(p)));
    let origin = center.map(|v| v - 8);
    let cells = WalkCells::gather(&memos, context, origin, [4, 4, 4], 4);
    let mut open = 0;
    for x in -8..=8 {
        for y in -8..=8 {
            let p = [
                (center[0] + x) as f64,
                (center[1] + y) as f64,
                center[2] as f64,
            ];
            assert_eq!(at(&small, p).min(0.4), at(&large, p).min(0.4));
            open += usize::from(at(&small, p) < 0.0);
            if x < 8 && y < 8 {
                let cell = (((y + 8) / 4 * 4 + 2) * 4 + (x + 8) / 4) as usize;
                let listed = if cells.any(cell) {
                    cells.at(cell, p)
                } else {
                    1.0
                };
                assert_eq!(listed, at(&large, p));
            }
        }
    }
    assert!(open > 0);
}
