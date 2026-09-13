use super::*;
use crate::block::Block;

#[test]
fn surface_buoyancy_converges_from_both_sides_without_overshoot() {
    let surface = 6.0;
    let target = surface - SURFACE_DRAFT;
    for start in [target - 2.0, target + 1.0] {
        let mut y = start;
        for _ in 0..200 {
            let before = target - y;
            let velocity = Immersion {
                fluid: Block::Water.fluid_def().unwrap(),
                surface_y: surface,
            }
            .vertical_velocity(0.0, y, Buoyancy::Surface, false, 0.05);
            y += velocity * 0.05;
            let after = target - y;
            assert!(
                before == 0.0 || before.signum() == after.signum() || after.abs() < 1e-6,
                "surface float crossed its target: {before} -> {after}"
            );
            assert!(
                after.abs() <= before.abs() + 1e-6,
                "surface float must converge monotonically: {before} -> {after}"
            );
        }
        assert!(
            (y - target).abs() < 1e-4,
            "surface float settles at the waterline from {start}: {y}"
        );
    }
}
