use super::*;
use crate::entity::fluid_fixture::{self, block, BRINE, SYRUP};
use crate::world::testutil::flat_world;

#[test]
fn bucket_rays_key_on_the_fluid_rows() {
    let root = fluid_fixture::stage("fluid-rays");
    crate::modding::tests::run_child_test(&root, "player::tests::fluid_rays::fluid_rays_inner");
}

#[test]
#[ignore = "child of bucket_rays_key_on_the_fluid_rows with fixture content"]
fn fluid_rays_inner() {
    fill_ray_stops_on_a_source_only_when_the_bucket_takes_its_fluid();
    pour_ray_stops_at_the_surface_of_any_fluid();
}

/// Eye three blocks over the top face of `cell`, looking straight down.
fn eye_over(cell: IVec3) -> (petramond_math::world_pos::WorldPos, Vec3) {
    let eye = petramond_math::world_pos::WorldPos::new(
        cell.x as f64 + 0.5,
        cell.y as f64 + 4.0,
        cell.z as f64 + 0.5,
    );
    (eye, Vec3::new(0.0, -1.0, 0.0))
}

/// The fill ray keys on the bucket's predicate over the fluid, not on one
/// concrete fluid: a bucket that takes any fluid stops on a syrup source, and
/// a brine-only bucket reads THROUGH that same syrup to the floor beneath — a
/// fluid the bucket does not take is as transparent as flow.
fn fill_ray_stops_on_a_source_only_when_the_bucket_takes_its_fluid() {
    let (brine, syrup) = (block(BRINE), block(SYRUP));
    let mut w = flat_world();
    let source = IVec3::new(8, 65, 8);
    assert!(w.set_block_world(source.x, source.y, source.z, syrup));
    let (eye, dir) = eye_over(source);

    let (any, _) = Player::raycast_fluid_sources(eye, dir, &w, |_| true).expect("any-fluid hit");
    assert_eq!(
        any.block, source,
        "the universal bucket's ray stops on the source"
    );

    let (brine_only, _) = Player::raycast_fluid_sources(eye, dir, &w, |f| f == brine)
        .expect("the floor still stops it");
    assert_eq!(
        brine_only.block,
        source - IVec3::Y,
        "a bucket that does not take syrup reads through it to the floor"
    );
}

/// The pour ray stops at the FIRST fluid cell of any fluid, so pouring at a
/// two-deep pool targets its surface cell (normal up), never the pool floor.
fn pour_ray_stops_at_the_surface_of_any_fluid() {
    for fluid in [block(BRINE), block(SYRUP)] {
        let mut w = flat_world();
        let bottom = IVec3::new(8, 65, 8);
        let top = bottom + IVec3::Y;
        assert!(w.set_block_world(bottom.x, bottom.y, bottom.z, fluid));
        assert!(w.set_block_world(top.x, top.y, top.z, fluid));
        let (eye, dir) = eye_over(top);

        let (hit, _) = Player::raycast_including_any_fluid(eye, dir, &w).expect("surface hit");
        assert_eq!(
            hit.block, top,
            "{fluid:?}: the pour ray stops at the surface cell"
        );
        assert_eq!(
            hit.normal,
            IVec3::Y,
            "{fluid:?}: entered through the top face"
        );
    }
}
