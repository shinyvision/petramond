use std::collections::BTreeMap;

use crate::block::{Block, CellView};
use crate::block_model::ModelCellState;
use crate::block_state::{EntityFront, StairState};
use crate::door::DoorState;
use crate::mathh::IVec3;
use crate::world::placement::authored::Turn;

use super::{Bounds, Cell, Template};

fn parse(value: serde_json::Value) -> Result<Template, String> {
    Template::parse(&value.to_string(), |key| {
        crate::registry::names().blocks.id(key).map(Block)
    })
}

fn fixture() -> serde_json::Value {
    serde_json::json!({
        "size": [10, 6, 10], "pivot": [4, 2, 4],
        "palette": {
            "rock": {"block": "petramond:stone"},
            "empty": {"block": "petramond:air"},
            "stairs": {"block": "petramond:oak_stairs", "state": {"facing": "east", "half": "top"}},
            "door": {"block": "petramond:oak_door", "state": {"facing": "west"}},
            "bed": {"block": "petramond:bed", "state": {"facing": "south"}},
            "chest": {"block": "petramond:chest", "state": {"facing": "east"}}
        },
        "blocks": [
            {"pos": [1, 1, 1], "palette": "stairs"},
            {"pos": [3, 1, 3], "palette": "door"},
            {"pos": [5, 1, 5], "palette": "bed"},
            {"pos": [7, 1, 7], "palette": "chest"},
            {"pos": [8, 1, 8], "palette": "empty"}
        ],
        "markers": [{"pos": [7, 1, 7], "key": "fixture:loot", "value": {"table": "fixture:tools"}}]
    })
}

#[test]
fn terrain_requirements_preserve_pivot_and_reject_unbounded_work() {
    let mut value = fixture();
    value["requirements"] = serde_json::json!([
        {"min": [1, -1, 2], "max": [8, -1, 7], "space": "solid"},
        {"min": [2, 1, 2], "max": [7, 4, 7], "space": "air"}
    ]);
    let info = parse(value.clone()).unwrap().info();
    assert_eq!(info.requirements[0].min, [-3, -3, -2]);
    assert_eq!(info.requirements[0].max, [4, -3, 3]);
    assert_eq!(info.requirements[1].space, mod_api::TerrainSpace::Air);
    value["requirements"][0]["min"][1] = (-2).into();
    assert!(parse(value.clone()).is_err());
    value["requirements"][0]["min"][1] = 0.into();
    assert!(parse(value.clone()).is_err());
    value["requirements"] = serde_json::Value::Array(vec![
        serde_json::json!(
            {"min": [0, 0, 0], "max": [9, 5, 9], "space": "water"}
        );
        8
    ]);
    assert!(parse(value).is_err());
}

fn cells(template: &Template, origin: IVec3, turn: Turn) -> BTreeMap<[i32; 3], Cell> {
    let placement = template.place(origin, turn).unwrap();
    let mut cells = BTreeMap::new();
    placement.visit(placement.bounds(), |pos, cell| {
        cells.insert(pos.to_array(), cell.clone());
    });
    cells
}

#[test]
fn rotations_preserve_family_state_and_complete_footprints() {
    let template = parse(fixture()).unwrap();
    let base = cells(&template, IVec3::ZERO, Turn::default());
    let origin = IVec3::new(-17, -33, 15);
    for turn in Turn::ALL {
        let rotated = cells(&template, origin, turn);
        assert_eq!(rotated.len(), base.len());
        for (pos, before) in &base {
            let pos = origin + turn.apply(IVec3::from(*pos));
            let after = &rotated[&pos.to_array()];
            assert_eq!(before.block, after.block);
            assert_eq!(before.data, after.data);
            if before.block == Block::OakStairs {
                let (a, b) = (
                    StairState::from_cell(before.state),
                    StairState::from_cell(after.state),
                );
                assert_eq!(b.facing, turn.facing(a.facing));
                assert_eq!(a.half, b.half);
            } else if before.block == Block::OakDoor {
                let a = <Option<DoorState>>::from_cell(before.state).unwrap();
                let b = <Option<DoorState>>::from_cell(after.state).unwrap();
                assert_eq!(b.facing, turn.facing(a.facing));
                assert_eq!(a.top, b.top);
            } else if before.block == Block::Bed {
                let (a, b) = (
                    ModelCellState::from_cell(before.state),
                    ModelCellState::from_cell(after.state),
                );
                assert_eq!(b.facing, turn.facing(a.facing));
                assert_eq!(a.offset, b.offset);
            } else if before.block == Block::Chest {
                assert_eq!(
                    EntityFront::from_cell(after.state).0,
                    turn.facing(EntityFront::from_cell(before.state).0)
                );
            }
        }
    }
}

#[test]
fn section_slices_equal_whole_placement_in_any_order() {
    let template = parse(fixture()).unwrap();
    for turn in Turn::ALL {
        let placement = template.place(IVec3::new(-16, -32, 16), turn).unwrap();
        let mut whole = BTreeMap::new();
        placement.visit(placement.bounds(), |pos, cell| {
            whole.insert(pos.to_array(), (cell.block, cell.state, cell.data.clone()));
        });
        let lo = placement.bounds().min.div_euclid(IVec3::splat(16));
        let hi = placement.bounds().max.div_euclid(IVec3::splat(16));
        let mut slices = BTreeMap::new();
        for x in (lo.x..=hi.x).rev() {
            for z in lo.z..=hi.z {
                for y in (lo.y..=hi.y).rev() {
                    let min = IVec3::new(x, y, z) * 16;
                    placement.visit(
                        Bounds {
                            min,
                            max: min + IVec3::splat(15),
                        },
                        |pos, cell| {
                            assert!(slices
                                .insert(pos.to_array(), (cell.block, cell.state, cell.data.clone()))
                                .is_none());
                        },
                    );
                }
            }
        }
        assert_eq!(whole, slices);
    }
}

#[test]
fn malformed_state_and_partial_objects_fail_compilation() {
    let mut value = fixture();
    value["palette"]["stairs"]["state"]["facign"] = "south".into();
    assert!(parse(value).err().unwrap().contains("facign"));
    let mut value = fixture();
    value["blocks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"pos": [3, 2, 3], "palette": "empty"}));
    assert!(parse(value).err().unwrap().contains("multi-cell"));
    let mut value = fixture();
    value["blocks"][1]["pos"] = serde_json::json!([3, 5, 3]);
    assert!(parse(value).err().unwrap().contains("footprint"));
    let mut value = fixture();
    value["palette"]["rock"]["block"] = "fixture:missing".into();
    assert!(parse(value).err().unwrap().contains("unknown block"));
}

#[test]
fn placements_reject_coordinate_overflow() {
    let template = parse(fixture()).unwrap();
    for turn in Turn::ALL {
        assert!(template.place(IVec3::splat(i32::MAX), turn).is_err());
        assert!(template.place(IVec3::splat(i32::MIN), turn).is_err());
    }
}

#[test]
fn connector_alignment_is_rotation_independent() {
    let piece = parse(serde_json::json!({
        "size": [3, 2, 5], "palette": {},
        "connectors": [
            {"name": "out", "kind": "fixture:passage", "pos": [2,0,2], "facing": "east"},
            {"name": "in", "kind": "fixture:passage", "pos": [0,0,2], "facing": "west"}
        ]
    }))
    .unwrap();
    for turn in Turn::ALL {
        let parent = piece.place(IVec3::new(-20, -17, 31), turn).unwrap();
        let child = parent.attach("out", &piece, "in").unwrap();
        let socket = &piece.connectors(turn)[0];
        let plug = &piece.connectors(child.turn())[1];
        assert_eq!(
            parent.origin() + socket.pos + socket.facing.dir(),
            child.origin() + plug.pos
        );
        assert_eq!(socket.facing.dir(), -plug.facing.dir());
        assert!(!parent.bounds().intersects(child.bounds()));
    }
}

#[test]
fn shipped_structures_compile() {
    super::validate_catalog();
}
