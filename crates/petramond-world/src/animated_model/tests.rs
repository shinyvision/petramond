use super::*;
use crate::block::CellCodec;
use crate::door::DoorState;
use crate::trapdoor::TrapdoorState;

const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

fn engine(key: &str) -> &'static AnimatedModelDef {
    by_key(key).unwrap_or_else(|| panic!("{key} ships in assets/animated_models.json"))
}

fn turn(p: [f32; 3], facing: Facing) -> [f32; 3] {
    use std::f32::consts::{FRAC_PI_2, PI};
    let yaw: f32 = match facing {
        Facing::South => 0.0,
        Facing::North => PI,
        Facing::East => FRAC_PI_2,
        Facing::West => -FRAC_PI_2,
    };
    let (s, c) = yaw.sin_cos();
    let (dx, dz) = (p[0] - 0.5, p[2] - 0.5);
    [0.5 + dx * c + dz * s, p[1], 0.5 - dx * s + dz * c]
}

fn posed_bounds(part: &ModelPart, open01: f32, facing: Facing) -> Aabb {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for corner in part.corners() {
        let p = match part.joint {
            Some(j) => j.rotate(corner, j.angle(open01).sin_cos()),
            None => corner,
        };
        let p = turn(p, facing);
        for a in 0..3 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    Aabb { min, max }
}

fn assert_same_box(drawn: Aabb, collides: Aabb, what: &str) {
    for a in 0..3 {
        assert!(
            (drawn.min[a] - collides.min[a]).abs() < 1e-5
                && (drawn.max[a] - collides.max[a]).abs() < 1e-5,
            "{what}: drawn {drawn:?} != collision {collides:?}"
        );
    }
}

#[test]
fn every_engine_model_loads() {
    for &key in ENGINE_MODEL_NAMES {
        let model = engine(key);
        assert_eq!(model.key, key);
        assert!(!model.variants().is_empty());
    }
}

#[test]
fn the_drawn_door_swing_lands_on_its_collision_slab_for_every_facing() {
    let lower = &engine("petramond:door").variant(0).parts[0];
    for facing in FACINGS {
        for open in [false, true] {
            let state = DoorState {
                facing,
                open,
                top: false,
            };
            assert_same_box(
                posed_bounds(lower, if open { 1.0 } else { 0.0 }, facing),
                crate::door::collision_boxes(state)[0],
                &format!("{facing:?} open={open}"),
            );
        }
    }
}

#[test]
fn the_drawn_trapdoor_swing_lands_on_its_collision_slab_for_every_pose() {
    let model = engine("petramond:trapdoor");
    for facing in FACINGS {
        for top in [false, true] {
            let panel = &model.variant(u8::from(top)).parts[0];
            for open in [false, true] {
                let state = TrapdoorState { facing, open, top };
                assert_same_box(
                    posed_bounds(panel, if open { 1.0 } else { 0.0 }, facing),
                    crate::trapdoor::collision_boxes(state)[0],
                    &format!("{facing:?} top={top} open={open}"),
                );
            }
        }
    }
}

#[test]
fn the_cull_box_holds_every_pose_under_every_facing() {
    for &key in ENGINE_MODEL_NAMES {
        for variant in engine(key).variants() {
            let cull = variant.cull;
            for part in variant.parts {
                for step in 0..=20 {
                    for facing in FACINGS {
                        let b = posed_bounds(part, step as f32 / 20.0, facing);
                        for a in 0..3 {
                            assert!(
                                b.min[a] >= cull.min[a] && b.max[a] <= cull.max[a],
                                "{key} step {step} {facing:?}: {b:?} outside {cull:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_family_poses_its_model_from_the_cell_state() {
    let door = |top| {
        DoorState {
            facing: Facing::West,
            open: true,
            top,
        }
        .to_cell()
    };
    let (_, pose) = Block::OakDoor
        .animated_pose(door(false))
        .expect("the lower half draws the door");
    assert_eq!(
        pose,
        AnimatedPose {
            facing: Facing::West,
            variant: 0,
            open: true
        }
    );
    assert!(
        Block::OakDoor.animated_pose(door(true)).is_none(),
        "the upper half is drawn by the lower"
    );
    let ceiling = TrapdoorState {
        facing: Facing::East,
        open: false,
        top: true,
    };
    let (_, pose) = Block::OakTrapdoor
        .animated_pose(ceiling.to_cell())
        .expect("a trapdoor draws itself");
    assert_eq!(pose.variant, 1, "a ceiling panel draws the ceiling variant");
    let front = crate::block_state::EntityFront(Facing::East).to_cell();
    let (_, pose) = Block::Chest
        .animated_pose(front)
        .expect("a chest draws itself");
    assert_eq!(
        pose.facing,
        Facing::East,
        "a chest faces its placement front"
    );
    assert!(
        !pose.open,
        "a chest's lid follows its viewers, not its state"
    );
}

#[test]
fn a_part_takes_its_rows_tiles() {
    let door = engine("petramond:door");
    let [top, bottom, side] = Block::OakDoor.tiles();
    let lower = door.variant(0).parts[0].tiles(Block::OakDoor);
    assert_eq!(lower, [side, side, side, side, bottom, bottom]);
    let upper = door.variant(0).parts[1].tiles(Block::OakDoor);
    assert_eq!(upper[4], top);
}

#[test]
fn an_out_of_range_variant_draws_the_first() {
    let door = engine("petramond:door");
    assert_eq!(door.variant(7), door.variant(0));
}

#[test]
fn malformed_rows_are_refused() {
    let layer = |part: &str| {
        format!(
            r#"{{"animated_models": [{{"model": "petramond:chest", "open_speed": 1,
                "variants": [{{"parts": [{part}]}}]}}]}}"#
        )
    };
    for (part, needle) in [
        (
            r#"{"from": [0,0,0], "to": [16,16,16], "faces": {"east": "chest_side"}}"#,
            "no tile for its 'west' face",
        ),
        (
            r#"{"from": [0,0,0], "to": [16,16,16], "faces": {"all": "$front"}}"#,
            "unknown row tile slot",
        ),
        (
            r#"{"from": [4,0,0], "to": [2,16,16], "faces": {"all": "$side"}}"#,
            "must be ordered",
        ),
        (
            r#"{"from": [0,0,0], "to": [16,16,16], "faces": {"all": "$side"},
                "joint": {"axis": "w", "pivot": [0,0,0], "open_degrees": 90}}"#,
            "unknown joint axis",
        ),
    ] {
        let text = layer(part);
        let err = parse_layers(&[text.as_str()])
            .err()
            .unwrap_or_else(|| panic!("{part} must be refused"));
        assert!(err.contains(needle), "{part}: {err}");
    }
}
