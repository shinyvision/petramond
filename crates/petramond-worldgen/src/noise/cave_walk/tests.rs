use super::*;

#[test]
fn spatial_pruning_preserves_density_and_box_intersections() {
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
        let point = p.map(f64::from);
        let direct = cuts
            .iter()
            .map(|cut| cut.density(point))
            .fold(1.0, f64::min);
        assert_eq!(index.at(&cuts, point), direct);
        let bounds = [p.map(|v| v - 4), p.map(|v| v + 4)];
        assert_eq!(
            index.intersects(&cuts, bounds),
            cuts.iter().any(|cut| cut.intersects(bounds))
        );
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
    let large = WalkField::gather(7, [center.map(|v| v - 80), center.map(|v| v + 80)]);
    let small = WalkField::gather(7, [center.map(|v| v - 8), center.map(|v| v + 8)]);
    let mut open = 0;
    for x in -8..=8 {
        for y in -8..=8 {
            let p = [
                (center[0] + x) as f64,
                (center[1] + y) as f64,
                center[2] as f64,
            ];
            // Positive distances can be culled; open space and its lining cannot.
            assert_eq!(small.at(p).min(0.4), large.at(p).min(0.4));
            open += usize::from(small.at(p) < 0.0);
        }
    }
    assert!(open > 0);
}
