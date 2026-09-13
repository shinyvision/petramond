use super::*;
use crate::entity::fluid_fixture::{self, block, BRINE};
use petramond_world::block::Block;

#[test]
fn shore_climbs_read_the_footprint_ahead() {
    let root = fluid_fixture::stage("shore-probe");
    crate::modding::tests::run_child_test(&root, "entity::shore::tests::shore_probe_inner");
}

#[test]
#[ignore = "child of shore_climbs_read_the_footprint_ahead with fixture content"]
fn shore_probe_inner() {
    a_ledge_under_the_leading_edge_counts_off_the_centre_line();
    a_ledge_without_headroom_is_no_shore();
    a_launch_stays_inside_the_velocity_slack();
}

const JUMP_SPEED: f32 = 8.0;

/// A brine surface at `y = 1` over the cell row `y = 0`.
fn brine_surface() -> Immersion {
    Immersion {
        fluid: block(BRINE).fluid_def().unwrap(),
        surface_y: 1.0,
    }
}

fn swimmer(pos: WorldPos) -> Swimmer {
    Swimmer {
        pos,
        vel_y: 0.0,
        half_width: 0.3,
        height: 1.8,
        gravity: 20.0,
        jump_speed: JUMP_SPEED,
    }
}

fn stone_at(cells: &'static [[i32; 3]]) -> impl Fn(i32, i32, i32) -> &'static [Aabb] {
    move |x, y, z| {
        if cells.contains(&[x, y, z]) {
            Block::Stone.collision_boxes()
        } else {
            &[]
        }
    }
}

/// The ledge lies beside the centre line but under the body's leading edge.
fn a_ledge_under_the_leading_edge_counts_off_the_centre_line() {
    let body = swimmer(WorldPos::new(0.6, -0.5, 0.8));
    let boxes = stone_at(&[[1, 0, 1]]);
    assert!(matches!(
        body.shore_climb(Vec3::X, brine_surface(), &boxes, &[]),
        Some(ShoreClimb::Launch(_))
    ));
}

fn a_ledge_without_headroom_is_no_shore() {
    let body = swimmer(WorldPos::new(0.6, -0.5, 0.5));
    let open = stone_at(&[[1, 0, 0]]);
    let roofed = stone_at(&[[1, 0, 0], [1, 2, 0]]);
    assert!(body
        .shore_climb(Vec3::X, brine_surface(), &open, &[])
        .is_some());
    assert_eq!(
        body.shore_climb(Vec3::X, brine_surface(), &roofed, &[]),
        None,
        "a body standing on the ledge would not fit under the roof"
    );
}

/// The server's claim envelope trusts a launch no faster than the jump
/// take-off plus the shared slack, however tall the ledge.
fn a_launch_stays_inside_the_velocity_slack() {
    let body = swimmer(WorldPos::new(0.6, -0.7, 0.5));
    let boxes = stone_at(&[[1, 0, 0], [1, 1, 0]]);
    let Some(ShoreClimb::Launch(speed)) = body.shore_climb(Vec3::X, brine_surface(), &boxes, &[])
    else {
        panic!("a two-high bank from deep in the fluid is still a shore");
    };
    assert!(speed <= JUMP_SPEED * VELOCITY_SLACK, "launch {speed}");
}
