use super::*;
use crate::block::Block;
use crate::mob::{def, Mob, MobDamageFeedback};

fn floor_at_zero(p: IVec3) -> bool {
    p.y < 0
}

fn owl_def() -> &'static MobDef {
    def(Mob::Owl)
}

fn default_feedback() -> MobDamageFeedback {
    MobDamageFeedback::default()
}

fn sheep_def() -> &'static MobDef {
    def(Mob::Sheep)
}

#[test]
fn gravity_settles_the_mob_on_the_floor() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
    for _ in 0..600 {
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
    }
    assert!(
        owl.pos.y >= -1e-3,
        "mob fell through the floor: {}",
        owl.pos.y
    );
    assert!(owl.pos.y < 0.05, "mob rests on the floor: {}", owl.pos.y);
    assert!(owl.on_ground());
}

#[test]
fn mob_body_rests_on_an_inset_block_top_not_the_cell_top() {
    // Model-aware body collision: a mob settling onto an INSET block (a chest, top at
    // 14/16) rests its feet on that real top, not the full-cube cell top (y = 1). The
    // mob body now collides through the shared `collision_boxes_at` shape (nav stays
    // cell-based, but that's a separate concern).
    let chest = crate::block::Block::Chest.collision_boxes();
    let chest_top = chest.iter().map(|b| b.max[1]).fold(0.0, f32::max);
    assert!(
        chest_top < 1.0,
        "the chest box must be inset (top {chest_top})"
    );
    let boxes = |_x: i32, y: i32, _z: i32| if y == 0 { chest } else { &[][..] };
    let solid = |c: IVec3| c.y == 0; // nav sees the chest cell as a unit obstacle
    let dry = |_: IVec3| false;
    let still = |_: Vec3| Vec3::ZERO;
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
    for _ in 0..600 {
        owl.integrate_with_flow(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            true,
            &boxes,
            &[],
            &[],
            &solid,
            &dry,
            &|_| None,
            &still,
        );
    }
    assert!(owl.on_ground(), "mob should be grounded on the chest");
    assert!(
        (owl.pos.y - chest_top).abs() < 0.02,
        "mob feet should rest on the chest top {chest_top}, got {}",
        owl.pos.y
    );
}

#[test]
fn grounded_mob_auto_steps_up_a_half_block() {
    // A grounded mob walking into a 0.5-tall ledge auto-climbs it (same STEP_HEIGHT as
    // the player), without needing a jump.
    let half_step = |x: i32, y: i32, _z: i32| -> &'static [crate::block::Aabb] {
        if y == 0 {
            Block::Stone.collision_boxes()
        } else if y == 1 && x >= 1 {
            &[crate::block::Aabb {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 0.5, 1.0],
            }]
        } else {
            &[]
        }
    };
    let solid = |c: IVec3| c.y == 0 || (c.y == 1 && c.x >= 1); // nav obstacle
    let dry = |_: IVec3| false;
    let still = |_: Vec3| Vec3::ZERO;
    let wish = Vec3::new(1.0, 0.0, 0.0);
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    for _ in 0..180 {
        owl.integrate_with_flow(
            1.0 / 60.0,
            owl_def(),
            wish,
            false,
            true,
            &half_step,
            &[],
            &[],
            &solid,
            &dry,
            &|_| None,
            &still,
        );
    }
    assert!(owl.pos.x > 1.2, "mob steps onto the ledge: x={}", owl.pos.x);
    assert!(
        owl.pos.y > 1.4,
        "mob rises onto the 0.5 ledge top: y={}",
        owl.pos.y
    );
}

#[test]
fn navigation_jump_keeps_steering_until_it_clears_a_full_block_step() {
    // A one-block navigation jump has an airborne phase where the body is still below
    // the ledge top and colliding with the block side. The mob must keep applying the
    // current route wish while rising, otherwise that side hit zeros horizontal
    // velocity and the jump stalls at the face.
    let solid = |c: IVec3| c.y < 1 || (c.x >= 1 && c.y < 2);
    let dry = |_: IVec3| false;
    let still = |_: Vec3| Vec3::ZERO;
    let wish = Vec3::new(1.0, 0.0, 0.0);
    let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);

    sheep.integrate_with_flow(
        0.05,
        sheep_def(),
        Vec3::ZERO,
        false,
        true,
        &boxes_of(&solid),
        &[],
        &[],
        &solid,
        &dry,
        &|_| None,
        &still,
    );
    assert!(sheep.on_ground(), "test starts from the lower floor");

    let mut left_ground = false;
    for _ in 0..80 {
        let can_steer = route_steering_supported(sheep.on_ground, false, sheep.vel.y);
        let jump = sheep.on_ground && sheep.pos.y < 1.5;
        sheep.integrate_with_flow(
            0.05,
            sheep_def(),
            wish,
            jump,
            can_steer,
            &boxes_of(&solid),
            &[],
            &[],
            &solid,
            &dry,
            &|_| None,
            &still,
        );
        left_ground |= !sheep.on_ground();
        if sheep.on_ground() && sheep.pos.y > 1.9 {
            break;
        }
    }

    assert!(left_ground, "the mob actually performed an airborne jump");
    assert!(
        sheep.on_ground() && sheep.pos.y > 1.9,
        "mob should land on the one-block step, pos {:?}",
        sheep.pos
    );
    assert!(
        sheep.pos.x + sheep_def().size.half_width > 1.0,
        "mob footprint should cross onto the step, pos {:?}",
        sheep.pos
    );
}

#[test]
fn wish_direction_drives_horizontal_motion_and_facing() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    // Settle on the ground first.
    owl.integrate(
        1.0 / 60.0,
        owl_def(),
        Vec3::ZERO,
        false,
        &floor_at_zero,
        &|_| false,
    );
    let x0 = owl.pos.x;
    for _ in 0..30 {
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
            &|_| false,
        );
    }
    assert!(
        owl.pos.x > x0 + 0.3,
        "wish +X should move the mob: {} -> {}",
        x0,
        owl.pos.x
    );
    assert!(owl.moving, "moving flag set while walking");
    // Faces +X: heading_yaw((+,0,0)) = atan2(-1, 0) = -PI/2.
    assert!(
        (wrap_angle(owl.yaw - (-PI / 2.0))).abs() < 0.2,
        "turns to face travel: {}",
        owl.yaw
    );
}

#[test]
fn airborne_sheep_carries_velocity_without_walk_steering() {
    let empty_boxes = |_x: i32, _y: i32, _z: i32| -> &'static [crate::block::Aabb] { &[] };
    let dry = |_: IVec3| false;
    let still = |_: Vec3| Vec3::ZERO;
    let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
    sheep.vel.x = 1.0;

    sheep.integrate_with_flow(
        1.0 / 60.0,
        sheep_def(),
        Vec3::new(-1.0, 0.0, 0.0),
        false,
        false,
        &empty_boxes,
        &[],
        &[],
        &dry,
        &dry,
        &|_| None,
        &still,
    );

    assert!(
        sheep.pos.x > 0.5,
        "falling should carry prior +X velocity instead of steering left: x {}",
        sheep.pos.x
    );
    assert!(
        sheep.vel.x > 0.0,
        "airborne walk wish must not overwrite carried velocity: vx {}",
        sheep.vel.x
    );
    assert!(
        !sheep.moving,
        "unsupported falling should not play the walk animation"
    );
}

#[test]
fn an_airborne_drive_cannot_replace_carry_or_yaw() {
    let empty_boxes = |_x: i32, _y: i32, _z: i32| -> &'static [crate::block::Aabb] { &[] };
    let dry = |_: IVec3| false;
    let still = |_: Vec3| Vec3::ZERO;
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.25, 1);
    owl.vel.x = 1.0;
    assert!(owl.set_drive(-5.0, 0.0, Some(1.5)));

    owl.integrate_with_flow(
        1.0 / 20.0,
        owl_def(),
        Vec3::ZERO,
        false,
        false,
        &empty_boxes,
        &[],
        &[],
        &dry,
        &dry,
        &|_| None,
        &still,
    );

    assert!(owl.pos.x > 0.5, "airborne carry wins over driven -X");
    assert_eq!(owl.yaw, 0.25, "airborne drive yaw is ignored too");
    assert!(owl.drive.is_none(), "the rejected intent still expires");
}

#[test]
fn jump_impulse_lifts_a_grounded_mob() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    owl.integrate(
        1.0 / 60.0,
        owl_def(),
        Vec3::ZERO,
        false,
        &floor_at_zero,
        &|_| false,
    );
    assert!(owl.on_ground());
    owl.integrate(
        1.0 / 60.0,
        owl_def(),
        Vec3::ZERO,
        true,
        &floor_at_zero,
        &|_| false,
    );
    assert!(!owl.on_ground(), "jump leaves the ground");
    assert!(owl.pos.y > 0.0, "jump raises the mob");
}

#[test]
fn idle_mob_is_not_moving() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..10 {
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
    }
    assert!(
        !owl.moving,
        "a still mob reports not moving (renders the rest pose)"
    );
}

#[test]
fn a_drive_intent_moves_the_mob_for_one_tick_then_expires() {
    // A mod's kinematic drive replaces the wish overwrite for exactly the
    // tick it was issued: the mob moves at the driven velocity with its
    // yaw set, does not read as walking, and — like the brain's wish —
    // the intent must be re-issued or the next tick's overwrite parks it.
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(owl.set_drive(2.0, 0.0, Some(1.0)));
    owl.integrate(
        1.0 / 20.0,
        owl_def(),
        Vec3::ZERO,
        false,
        &floor_at_zero,
        &|_| false,
    );
    assert!(owl.pos.x > 0.5, "the drive velocity moved the mob");
    assert!(
        (owl.yaw - 1.0).abs() < 1e-5,
        "the drive yaw is absolute: {}",
        owl.yaw
    );
    assert!(!owl.moving, "driven is not walking (no walk anim/noise)");

    let x = owl.pos.x;
    owl.integrate(
        1.0 / 20.0,
        owl_def(),
        Vec3::ZERO,
        false,
        &floor_at_zero,
        &|_| false,
    );
    assert_eq!(owl.pos.x, x, "an un-renewed drive expires — the mob parks");
    assert!(
        (owl.yaw - 1.0).abs() < 1e-5,
        "nothing fights the driven yaw while idle: {}",
        owl.yaw
    );
}

#[test]
fn knockback_stagger_overrides_a_drive_intent() {
    // A punched vehicle takes its knockback: the decaying knockback owns
    // horizontal velocity for the stagger, the drive is consumed unused.
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    let from = Vec3::new(2.0, 0.0, 0.5); // hit from +X: knockback pushes -X
    owl.damage(1.0, Some(from), true, None, &default_feedback());
    assert!(owl.set_drive(5.0, 0.0, Some(1.0)));
    owl.integrate(
        1.0 / 20.0,
        owl_def(),
        Vec3::ZERO,
        false,
        &floor_at_zero,
        &|_| false,
    );
    assert!(
        owl.pos.x < 0.5,
        "knockback wins over the drive during the stagger: x {}",
        owl.pos.x
    );
    assert_eq!(owl.yaw, 0.0, "stagger rejects the drive yaw as well");
    assert!(owl.drive.is_none(), "the rejected intent still expires");
}

#[test]
fn knockback_pushes_away_and_overrides_the_wish() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    // Settle on the floor first.
    owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
        false
    });
    let x0 = owl.pos.x;
    // Hit from the +X side → knockback toward -X. This is the key invariant: the
    // knockback survives `integrate`'s per-tick wish-velocity overwrite.
    assert!(!owl.damage(
        1.0,
        Some(Vec3::new(5.0, 0.0, 0.5)),
        true,
        None,
        &default_feedback()
    ));
    // Wish toward +X (toward the attacker); the knockback must win during the stagger.
    for _ in 0..4 {
        owl.integrate(
            0.05,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
            &|_| false,
        );
    }
    assert!(
        owl.pos.x < x0 - 0.05,
        "knocked back -X despite wishing +X: {x0} -> {}",
        owl.pos.x
    );
    assert!(!owl.moving, "a staggered mob doesn't read as walking");
}

#[test]
fn a_submerged_mob_swims_up_instead_of_sinking() {
    // Solid bed below y==0, water filling y in 0..=5. Start the mob submerged at
    // y==1: buoyancy should lift it over a few ticks (gravity alone would sink it).
    let solid = |c: IVec3| c.y < 0;
    let water = |c: IVec3| (0..=5).contains(&c.y);
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    let y0 = owl.pos.y;
    for _ in 0..20 {
        owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &solid, &water);
    }
    assert!(
        owl.pos.y > y0,
        "a submerged mob rises toward the surface: {y0} -> {}",
        owl.pos.y
    );
}

#[test]
fn surface_buoyancy_converges_from_both_sides_without_overshoot() {
    let surface = 6.0;
    let target = surface - SURFACE_DRAFT;
    for start in [target - 2.0, target + 1.0] {
        let mut y = start;
        for _ in 0..200 {
            let before = target - y;
            let velocity = surface_vertical_velocity(0.0, y, Some(surface), 0.05);
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

#[test]
fn a_surface_body_out_of_water_falls_under_gravity() {
    let mut velocity = 0.0;
    for _ in 0..3 {
        let next = surface_vertical_velocity(velocity, 10.0, None, 0.05);
        assert!(next < velocity, "gravity accelerates the dry hull downward");
        velocity = next;
    }
}

#[test]
fn a_mob_bobs_up_and_down_through_the_water_surface_like_the_player() {
    // Water fills y in 0..=5 (surface at y==6) over a solid bed at y<0. The mob
    // swims up, breaks the surface, gravity pulls it back, it re-enters and rises
    // again — a real bob through the waterline (not a dead float, not a wiggle that
    // never re-enters). Run the real 20 TPS step.
    let solid = |c: IVec3| c.y < 0;
    let water = |c: IVec3| (0..=5).contains(&c.y);
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    // Let it rise to the surface and get into the bob.
    for _ in 0..100 {
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &solid, &water);
    }
    // Over the next couple of seconds it must move both up (swim) and down
    // (gravity), and stay in a sane band around the surface.
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut went_up, mut went_down) = (false, false);
    for _ in 0..120 {
        let before = owl.pos.y;
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &solid, &water);
        let dy = owl.pos.y - before;
        went_up |= dy > 0.01;
        went_down |= dy < -0.01;
        lo = lo.min(owl.pos.y);
        hi = hi.max(owl.pos.y);
    }
    assert!(
        went_up && went_down,
        "bobs both up and down (up {went_up}, down {went_down})"
    );
    assert!(hi > 5.5, "rises up to/through the surface: hi {hi}");
    assert!(
        (4.0..=7.0).contains(&lo) && (4.0..=7.0).contains(&hi),
        "stays at the waterline: {lo}..{hi}"
    );
}

#[test]
fn a_swimming_mob_climbs_out_onto_an_adjacent_ledge() {
    // A shore the climb-boost can actually clear: water (cells y in 0..SURFACE) over a
    // bed at y<0, with land at x>=1 whose top is AT the waterline. The swim climb-boost
    // (`SWIM_CLIMB`, fired by `ledge_ahead`) lifts the mob's feet just over the surface
    // so it steps out onto the land instead of hugging the shore forever. How high the
    // boost reaches depends on the (tunable) swim constants, so the land is kept at the
    // waterline and the checks derive from the owl's own size + this geometry — no swim
    // numbers are baked in. (The original test hard-coded a 1-block ledge, which needs
    // a far stronger boost than the tuned `SWIM_CLIMB` and so never passed.)
    const SURFACE: i32 = 4; // top of the water (and of the land it climbs onto)
    const SHORE: f32 = 1.0; // land starts at world x = 1
    let solid = |c: IVec3| c.y < 0 || (c.x >= 1 && c.y < SURFACE);
    let water = |c: IVec3| c.x <= 0 && (0..SURFACE).contains(&c.y);
    let half = owl_def().size.half_width;
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    for _ in 0..300 {
        owl.integrate(
            0.05,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &solid,
            &water,
        );
    }
    assert!(
        owl.on_ground(),
        "settled on the land, not still bobbing in the water: y {}",
        owl.pos.y
    );
    assert!(
        owl.pos.y >= SURFACE as f32 - 0.05,
        "rests up at the land surface, out of the water: y {}",
        owl.pos.y
    );
    assert!(
        owl.pos.x + half > SHORE,
        "climbed past the shore onto the land: x {}",
        owl.pos.x
    );
}

#[test]
fn swim_climb_does_not_boost_toward_a_ledge_above_reach() {
    const SURFACE: i32 = 4;
    // Land top is one block above the waterline. From the submerged start pose this
    // is not yet reachable; the mob must swim up first instead of getting a cliff
    // boost from below.
    let solid = |c: IVec3| c.y < 0 || (c.x >= 1 && c.y < SURFACE + 1);
    let water = |c: IVec3| c.x <= 0 && (0..SURFACE).contains(&c.y);
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, SURFACE as f32 - 0.7, 0.5), 0.0, 1);
    assert!(
        !owl.ledge_ahead(Vec3::new(1.0, 0.0, 0.0), owl_def().size.half_width, &solid),
        "ledge top is too far above the mob's current feet"
    );
    let y0 = owl.pos.y;
    owl.integrate(
        0.05,
        owl_def(),
        Vec3::new(1.0, 0.0, 0.0),
        false,
        &solid,
        &water,
    );
    assert!(
        owl.pos.y < y0 + 0.1,
        "uses normal swim rise, not the ledge boost: {y0} -> {}",
        owl.pos.y
    );
}

#[test]
fn a_mob_in_flowing_water_is_carried_downstream() {
    // Water fills y in 0..=5 over a solid bed at y<0, with a current heading +X
    // everywhere. A mob sitting in it with no wish to move must still drift
    // downstream — like the player and dropped items do.
    let solid = |c: IVec3| c.y < 0;
    let water = |c: IVec3| (0..=5).contains(&c.y);
    let flow = |_: Vec3| Vec3::new(1.0, 0.0, 0.0);
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    let x0 = owl.pos.x;
    for _ in 0..60 {
        owl.integrate_with_flow(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            true,
            &boxes_of(&solid),
            &[],
            &[],
            &solid,
            &water,
            &|_| None,
            &flow,
        );
    }
    assert!(
        owl.pos.x > x0 + 0.3,
        "the current carries the mob downstream: {x0} -> {}",
        owl.pos.x
    );

    // Still water (no current) leaves an idle mob where it is — proving it's the flow
    // doing the carrying, not stray drift.
    let still = |_: Vec3| Vec3::ZERO;
    let mut calm = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    for _ in 0..60 {
        calm.integrate_with_flow(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            true,
            &boxes_of(&solid),
            &[],
            &[],
            &solid,
            &water,
            &|_| None,
            &still,
        );
    }
    assert!(
        (calm.pos.x - 0.5).abs() < 1e-3,
        "no current → no horizontal drift: x {}",
        calm.pos.x
    );
}
