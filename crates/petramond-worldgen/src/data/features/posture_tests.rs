use super::RawShape;
use serde_json::{json, Value};

fn resolves(value: Value) -> bool {
    serde_json::from_value::<RawShape>(value)
        .unwrap()
        .resolve()
        .is_ok()
}

#[test]
fn posture_rows_reject_geometry_that_cannot_be_replayed_or_supported() {
    let canopy = json!({"canopy": {
        "log":"petramond:birch_log", "leaf":"petramond:birch_leaves",
        "height":[10,15], "split":0.5, "lean":[0,3], "tip_height":[-1,2],
        "limbs":[2,4], "reach":[2,4], "tip_radius":[2,3], "crown_radius":3, "round":0.5
    }});
    assert!(resolves(canopy.clone()));
    for (key, invalid) in [
        ("lean", json!([0, i32::MAX])),
        ("lean", json!([3, 1])),
        ("tip_height", json!([-30, 0])),
        ("tip_radius", json!([2, 12])),
        ("split", json!(1.0)),
    ] {
        let mut bad = canopy.clone();
        bad["canopy"][key] = invalid;
        assert!(!resolves(bad), "accepted invalid {key}");
    }

    let oak = json!({"blocky_oak": {
        "log":"petramond:oak_log", "leaf":"petramond:oak_leaves",
        "height":[12,16], "levels":[2,3], "reach_min":3, "reach_max":[4,6],
        "roots":[3,4], "root_reach":[2,4],
        "lean":[1,3], "branch_rise":[0.2,0.5], "crown_taper":0.3
    }});
    assert!(resolves(oak.clone()));
    for (key, invalid) in [
        ("lean", json!([0, i32::MAX])),
        ("branch_rise", json!([0.8, 0.2])),
        ("crown_taper", json!(-1.0)),
        ("reach_max", json!([4, i32::MAX])),
    ] {
        let mut bad = oak.clone();
        bad["blocky_oak"][key] = invalid;
        assert!(!resolves(bad), "accepted invalid {key}");
    }
}
