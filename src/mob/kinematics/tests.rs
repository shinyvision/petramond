use super::*;
use crate::mob::{def, Mob, MobDamageFeedback};
use petramond_math::math::IVec3;
use petramond_world::block::Block;

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
        owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
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
fn zero_gravity_preserves_vertical_drive_but_still_collides() {
    let (text, _) = petramond_world::assets::read_base_text("mobs.json").unwrap();
    let mut rows: serde_json::Value = serde_json::from_str(&text).unwrap();
    for row in rows["mobs"].as_array_mut().unwrap() {
        row["gravity_scale"] = serde_json::json!(0.0);
        row["air_control"] = serde_json::json!(true);
    }
    let text = rows.to_string();
    let table = crate::mob::load::parse_layers(&[&text]).unwrap();
    let d = table.defs.iter().find(|d| d.mob == Mob::Owl).unwrap();
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
    owl.vel.y = -1.0;
    owl.integrate(0.05, d, Vec3::ZERO, false, &floor_at_zero);
    assert!((owl.vel.y + 1.0).abs() < 1e-6);
    assert!((owl.pos.y - 4.95).abs() < 1e-5);
    let mut touched_floor = false;
    for _ in 0..120 {
        owl.integrate(0.05, d, Vec3::ZERO, false, &floor_at_zero);
        touched_floor |= owl.on_ground();
    }
    assert!(touched_floor);
    assert!(owl.pos.y.abs() < 0.01);
    assert_eq!(owl.vel.y, 0.0);
}

#[test]
fn a_body_embedded_in_a_grown_column_slides_out_sideways_without_bobbing() {
    // A trunk grew around the sheep (a door shut on it, ...): the foot heal
    // lifts it by its cap, gravity drops it back through the box it still
    // overlaps, forever. It must leave sideways, with the feet never rising.
    let full = Block::Stone.collision_boxes();
    // Floor at y < 0; a 3-high column in cell (0, 0..3, 0).
    let boxes = |x: i32, y: i32, z: i32| {
        if y < 0 || (x == 0 && z == 0 && (0..3).contains(&y)) {
            full
        } else {
            &[][..]
        }
    };
    let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.4, 0.0, 0.5), 0.0, 1);
    let mut peak = 0.0f32;
    for _ in 0..40 {
        sheep.integrate_locomotion(
            0.05,
            sheep_def(),
            Locomotion {
                wish: Vec3::ZERO,
                jump: false,
                can_steer: true,
            },
            &Surroundings::dry(&boxes),
        );
        peak = peak.max(sheep.pos.y);
    }
    let hw = sheep_def().size.half_width;
    assert!(
        sheep.pos.x + hw <= 0.0 + 1e-3 || sheep.pos.x - hw >= 1.0 - 1e-3,
        "the body left the column sideways: x {}",
        sheep.pos.x
    );
    assert!(peak < 0.05, "the feet never lifted (bob): peak {peak}");
    assert!(sheep.on_ground());
}

#[test]
fn mob_body_rests_on_an_inset_block_top_not_the_cell_top() {
    // Model-aware body collision: a mob settling onto an INSET block (a chest, top at
    // 14/16) rests its feet on that real top, not the full-cube cell top (y = 1). The
    // mob body now collides through the shared `collision_boxes_at` shape (nav stays
    // cell-based, but that's a separate concern).
    let chest = petramond_world::block::Block::Chest.collision_boxes();
    let chest_top = chest.iter().map(|b| b.max[1]).fold(0.0, f32::max);
    assert!(
        chest_top < 1.0,
        "the chest box must be inset (top {chest_top})"
    );
    let boxes = |_x: i32, y: i32, _z: i32| if y == 0 { chest } else { &[][..] };
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
    for _ in 0..600 {
        owl.integrate_locomotion(
            1.0 / 60.0,
            owl_def(),
            Locomotion {
                wish: Vec3::ZERO,
                jump: false,
                can_steer: true,
            },
            &Surroundings::dry(&boxes),
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
    let half_step = |x: i32, y: i32, _z: i32| -> &'static [petramond_world::block::Aabb] {
        if y == 0 {
            Block::Stone.collision_boxes()
        } else if y == 1 && x >= 1 {
            &[petramond_world::block::Aabb {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 0.5, 1.0],
            }]
        } else {
            &[]
        }
    };
    let wish = Vec3::new(1.0, 0.0, 0.0);
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
    for _ in 0..180 {
        owl.integrate_locomotion(
            1.0 / 60.0,
            owl_def(),
            Locomotion {
                wish,
                jump: false,
                can_steer: true,
            },
            &Surroundings::dry(&half_step),
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
    let wish = Vec3::new(1.0, 0.0, 0.0);
    let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);

    sheep.integrate_locomotion(
        0.05,
        sheep_def(),
        Locomotion {
            wish: Vec3::ZERO,
            jump: false,
            can_steer: true,
        },
        &Surroundings::dry(&boxes_of(&solid)),
    );
    assert!(sheep.on_ground(), "test starts from the lower floor");

    let mut left_ground = false;
    for _ in 0..80 {
        let can_steer = route_steering_supported(sheep.on_ground, false, sheep.vel.y);
        let jump = sheep.on_ground && sheep.pos.y < 1.5;
        sheep.integrate_locomotion(
            0.05,
            sheep_def(),
            Locomotion {
                wish,
                jump,
                can_steer,
            },
            &Surroundings::dry(&boxes_of(&solid)),
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
    owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
    let x0 = owl.pos.x;
    for _ in 0..30 {
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
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
    let empty_boxes =
        |_x: i32, _y: i32, _z: i32| -> &'static [petramond_world::block::Aabb] { &[] };
    let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
    sheep.vel.x = 1.0;

    sheep.integrate_locomotion(
        1.0 / 60.0,
        sheep_def(),
        Locomotion {
            wish: Vec3::new(-1.0, 0.0, 0.0),
            jump: false,
            can_steer: false,
        },
        &Surroundings::dry(&empty_boxes),
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
    let empty_boxes =
        |_x: i32, _y: i32, _z: i32| -> &'static [petramond_world::block::Aabb] { &[] };
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.25, 1);
    owl.vel.x = 1.0;
    assert!(owl.set_drive(DriveIntent {
        horizontal: Some([-5.0, 0.0]),
        vertical: None,
        yaw: Some(1.5),
        while_walking: false
    }));

    owl.integrate_locomotion(
        1.0 / 20.0,
        owl_def(),
        Locomotion {
            wish: Vec3::ZERO,
            jump: false,
            can_steer: false,
        },
        &Surroundings::dry(&empty_boxes),
    );

    assert!(owl.pos.x > 0.5, "airborne carry wins over driven -X");
    assert_eq!(owl.yaw, 0.25, "airborne drive yaw is ignored too");
    assert!(owl.drive.is_none(), "the rejected intent still expires");
}

#[test]
fn jump_impulse_lifts_a_grounded_mob() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
    assert!(owl.on_ground());
    owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, true, &floor_at_zero);
    assert!(!owl.on_ground(), "jump leaves the ground");
    assert!(owl.pos.y > 0.0, "jump raises the mob");
}

#[test]
fn idle_mob_is_not_moving() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..10 {
        owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
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
    assert!(owl.set_drive(DriveIntent {
        horizontal: Some([2.0, 0.0]),
        vertical: None,
        yaw: Some(1.0),
        while_walking: false
    }));
    owl.integrate(1.0 / 20.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
    assert!(owl.pos.x > 0.5, "the drive velocity moved the mob");
    assert!(
        (owl.yaw - 1.0).abs() < 1e-5,
        "the drive yaw is absolute: {}",
        owl.yaw
    );
    assert!(!owl.moving, "driven is not walking (no walk anim/noise)");

    let x = owl.pos.x;
    owl.integrate(1.0 / 20.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
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
    assert!(owl.set_drive(DriveIntent {
        horizontal: Some([5.0, 0.0]),
        vertical: None,
        yaw: Some(1.0),
        while_walking: false
    }));
    owl.integrate(1.0 / 20.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
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
    owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero);
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
        );
    }
    assert!(
        owl.pos.x < x0 - 0.05,
        "knocked back -X despite wishing +X: {x0} -> {}",
        owl.pos.x
    );
    assert!(!owl.moving, "a staggered mob doesn't read as walking");
}

/// The generic velocity seam a pack authors a gait (a hop) from: a VERTICAL
/// drive composes with the brain's own walking — launch, arc, the walking
/// expression carried through the unsteered descent, and the walk clip
/// re-phased FORWARD onto a cycle boundary at each takeoff. No gait
/// vocabulary exists engine-side; this drives exactly what a mod does.
#[test]
fn a_vertical_drive_launch_composes_with_walking_and_carries_the_gait() {
    let d = owl_def();
    let solid = floor_at_zero;
    let named = [crate::mob::model_meta::NamedAnimMeta {
        name: "walk".into(),
        length: 0.5,
        looping: true,
    }];
    let dt = 1.0 / 60.0;
    let mut mob = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);

    // Walk +X under the real steering gate, issuing the launch the way a mod
    // does: a vertical-only drive latched whenever the LAST tick ended
    // grounded and walking (the mod's tick system reads post-move state and
    // its intent is consumed by the next integration).
    let mut launches = 0;
    let mut descent_ticks = 0;
    let mut unmoving_descent_ticks = 0;
    let mut last_anim = 0.0f32;
    for _ in 0..600 {
        let was_grounded = mob.on_ground();
        if was_grounded && mob.vel().x * mob.vel().x + mob.vel().z * mob.vel().z > 0.25 {
            assert!(mob.set_drive(DriveIntent {
                horizontal: None,
                vertical: Some(4.6),
                yaw: None,
                while_walking: true,
            }));
        }
        let can_steer = route_steering_supported(mob.on_ground(), false, mob.vel().y);
        let wish = if can_steer {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::ZERO
        };
        mob.integrate_locomotion(
            dt,
            d,
            Locomotion {
                wish,
                jump: false,
                can_steer,
            },
            &Surroundings::dry(&boxes_of(&solid)),
        );
        let launched = was_grounded && !mob.on_ground() && mob.vel().y > 0.0;
        mob.apply_expression(dt, d, &named, &crate::mob::brain::BehaviorOutput::default());
        if launched {
            launches += 1;
            let phase = (mob.anim_time - d.walk_anim_rate * dt).rem_euclid(0.5);
            assert!(
                phase < 1e-3 || 0.5 - phase < 1e-3,
                "a takeoff starts a walk-clip cycle (phase {phase})"
            );
        }
        assert!(
            mob.anim_time >= last_anim,
            "the walk-clip clock only moves FORWARD ({last_anim} -> {}) — a backward \
             jump sweeps the clip in reverse on replicas",
            mob.anim_time
        );
        last_anim = mob.anim_time;
        if !mob.on_ground() && mob.vel().y < 0.0 {
            descent_ticks += 1;
            if !mob.moving {
                unmoving_descent_ticks += 1;
            }
        }
    }
    assert!(launches >= 3, "the drive keeps launching hops: {launches}");
    assert!(descent_ticks > 0, "hops have a falling half");
    assert_eq!(
        unmoving_descent_ticks, 0,
        "a descending arc that began as a walk still reads as walking"
    );
    assert!(
        mob.pos.x > 5.0,
        "hopping still covers ground: x={}",
        mob.pos.x
    );

    // A navigation step jump keeps priority over the vertical drive on the
    // tick both fire: launched at the full jump_speed, which clears the
    // one-block ledge the route depends on.
    let mut jumper = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..60 {
        jumper.integrate(dt, d, Vec3::ZERO, false, &solid);
    }
    assert!(jumper.on_ground());
    assert!(jumper.set_drive(DriveIntent {
        horizontal: None,
        vertical: Some(4.6),
        yaw: None,
        while_walking: false
    }));
    jumper.integrate(dt, d, Vec3::new(1.0, 0.0, 0.0), true, &solid);
    assert!(
        jumper.vel().y > 4.6,
        "a nav jump launches at jump_speed, not the drive: vy={}",
        jumper.vel().y
    );

    // A HORIZONTAL drive keeps its vehicle semantics: driven is not walking.
    let mut driven = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..60 {
        driven.integrate(dt, d, Vec3::ZERO, false, &solid);
    }
    assert!(driven.set_drive(DriveIntent {
        horizontal: Some([2.0, 0.0]),
        vertical: None,
        yaw: None,
        while_walking: false
    }));
    driven.integrate(dt, d, Vec3::ZERO, false, &solid);
    assert!(
        !driven.moving,
        "a horizontally-driven mob never reads as walking"
    );
    assert!(driven.vel().x > 1.9, "the drive velocity applies");
}

/// A shove is not a walk: the soft entity push moves the body but never sets
/// `moving` — mod gait policies (the rabbit hop) gate launches on the
/// deliberate-locomotion fact, so a pushed-around mob must slide, not gait.
#[test]
fn a_shoved_mob_moves_without_reading_as_walking() {
    let d = owl_def();
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..60 {
        owl.integrate(1.0 / 60.0, d, Vec3::ZERO, false, &floor_at_zero);
    }
    assert!(owl.on_ground());

    owl.set_push(Vec3::new(3.0, 0.0, 0.0));
    owl.integrate(1.0 / 60.0, d, Vec3::ZERO, false, &floor_at_zero);
    assert!(owl.pos.x > 0.5, "the push displaces the body");
    assert!(!owl.moving, "a shove never reads as walking");

    // A WALKING mob that also gets shoved keeps its intent.
    owl.set_push(Vec3::new(0.0, 0.0, 3.0));
    owl.integrate(
        1.0 / 60.0,
        d,
        Vec3::new(1.0, 0.0, 0.0),
        false,
        &floor_at_zero,
    );
    assert!(owl.moving, "walking while shoved is still walking");
}

/// A walking-gated drive validates its premise at CONSUMPTION: the intent is
/// decided from last tick's state, and if the walk it was premised on ended
/// in between (arrival, an abandoned route), it must drop whole — or every
/// wander leg ends with one stale in-place bounce at the destination.
#[test]
fn a_walking_gated_drive_drops_when_the_walk_ended_before_consumption() {
    let d = owl_def();
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..60 {
        owl.integrate(1.0 / 60.0, d, Vec3::ZERO, false, &floor_at_zero);
    }
    assert!(owl.on_ground());

    // Premise broken: latched while walking, consumed on an idle tick.
    assert!(owl.set_drive(DriveIntent {
        horizontal: None,
        vertical: Some(4.6),
        yaw: None,
        while_walking: true,
    }));
    owl.integrate(1.0 / 60.0, d, Vec3::ZERO, false, &floor_at_zero);
    assert!(
        owl.on_ground() && owl.vel().y <= 0.0,
        "the stale gated intent is dropped: no parting bounce"
    );
    assert!(owl.drive.is_none(), "the dropped intent still expires");

    // Premise holds: same intent on a walking tick launches.
    assert!(owl.set_drive(DriveIntent {
        horizontal: None,
        vertical: Some(4.6),
        yaw: None,
        while_walking: true,
    }));
    owl.integrate(
        1.0 / 60.0,
        d,
        Vec3::new(1.0, 0.0, 0.0),
        false,
        &floor_at_zero,
    );
    assert!(
        !owl.on_ground() && owl.vel().y > 0.0,
        "a gated intent whose premise holds launches normally"
    );

    // An UNGATED intent stays unconditional (a startle jump from standstill).
    let mut idle = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    for _ in 0..60 {
        idle.integrate(1.0 / 60.0, d, Vec3::ZERO, false, &floor_at_zero);
    }
    assert!(idle.set_drive(DriveIntent {
        horizontal: None,
        vertical: Some(4.6),
        yaw: None,
        while_walking: false,
    }));
    idle.integrate(1.0 / 60.0, d, Vec3::ZERO, false, &floor_at_zero);
    assert!(
        !idle.on_ground() && idle.vel().y > 0.0,
        "an ungated launch from standstill still applies"
    );
}

#[test]
fn a_kinematic_pose_is_written_verbatim_and_a_released_body_flies_on() {
    // A mod-authored pose replaces the tick's motion wholesale: the body is
    // exactly where it was put, tilted as it was told, and the placement's
    // implied velocity survives it — so a body the mod stops placing keeps
    // that velocity into the engine's own integration instead of being
    // zeroed as a standing body's would be.
    let mut cart = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    let dt = 1.0 / 20.0;
    assert!(cart.set_kinematic(KinematicPose {
        pos: Vec3::new(0.9, 0.0, 0.5),
        yaw: 1.0,
        tilt: Tilt::new(0.4, -0.5),
    }));
    let pose = cart.kinematic.take().expect("latched");
    cart.place_kinematic(dt, pose);
    assert_eq!(cart.pos, Vec3::new(0.9, 0.0, 0.5));
    assert_eq!((cart.yaw, cart.tilt), (1.0, Tilt::new(0.4, -0.5)));
    assert!(
        (cart.vel.x - 0.4 / dt).abs() < 1e-3,
        "implied velocity: {}",
        cart.vel
    );
    assert!(!cart.moving, "authored motion is not a walk");
    assert!(
        cart.take_fall_distance().is_none(),
        "re-anchored: no fall latched"
    );

    // Released: the next ordinary integration — steering gated exactly as
    // the tick gates it, off the airborne flag the placement left — carries
    // the implied velocity instead of parking the body like a standing one.
    let x = cart.pos.x;
    let can_steer = route_steering_supported(cart.on_ground, false, cart.vel.y);
    assert!(!can_steer, "a placed body is left airborne");
    cart.integrate_locomotion(
        dt,
        owl_def(),
        Locomotion {
            wish: Vec3::ZERO,
            jump: false,
            can_steer,
        },
        &Surroundings::dry(&boxes_of(&floor_at_zero)),
    );
    assert!(cart.pos.x > x + 0.3, "the body flew on: {}", cart.pos.x);
    assert!(
        cart.kinematic.is_none(),
        "a pose is consumed by the tick it was issued for"
    );
    // Back in the engine's hands the body settles level over a few ticks —
    // eased, never snapped, and never left lying tilted where it landed.
    let before = cart.tilt;
    cart.level_body(dt);
    assert!(
        cart.tilt.pitch < before.pitch && cart.tilt.pitch > 0.0,
        "eases: {:?}",
        cart.tilt
    );
    assert!(cart.tilt.roll > before.roll && cart.tilt.roll < 0.0);
    for _ in 0..20 {
        cart.level_body(dt);
    }
    assert!(cart.tilt.is_level(), "settles: {:?}", cart.tilt);
}

#[test]
fn a_kinematic_pose_is_refused_on_a_dead_body_and_discarded_with_the_drive() {
    let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(owl.set_kinematic(KinematicPose {
        pos: Vec3::ZERO,
        yaw: 0.0,
        tilt: Tilt::LEVEL
    }));
    owl.clear_drive();
    assert!(owl.kinematic.is_none(), "a frozen tick discards the pose");
    owl.damage(100.0, None, true, None, &default_feedback());
    assert!(!owl.set_kinematic(KinematicPose {
        pos: Vec3::ZERO,
        yaw: 0.0,
        tilt: Tilt::LEVEL
    }));
}

#[test]
fn brain_speed_scale_changes_horizontal_travel_and_gait_together() {
    let mut normal = Instance::new(Mob::Sheep, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
    let mut hurried = Instance::new(Mob::Sheep, normal.pos, 0.0, 1);
    normal.on_ground = true;
    hurried.on_ground = true;
    let ratio = 1.75;
    hurried.walk_speed_scale = ratio;
    for mob in [&mut normal, &mut hurried] {
        mob.integrate(
            0.05,
            sheep_def(),
            Vec3::new(0.0, 0.0, -1.0),
            false,
            &floor_at_zero,
        );
        mob.apply_expression(
            0.05,
            sheep_def(),
            &[],
            &crate::mob::brain::BehaviorOutput::default(),
        );
    }
    assert!((hurried.vel.z - normal.vel.z * ratio).abs() < 1e-5);
    assert!((hurried.anim_time - normal.anim_time * ratio).abs() < 1e-5);
    assert_eq!(hurried.vel.y, normal.vel.y);
}
