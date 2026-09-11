use super::*;

#[test]
fn site_bounds_intersect_the_envelope_and_reject_position_dependencies() {
    let raw: RawBounds = serde_json::from_str(
        r#"{
        "min":[["sub","center_x",20],["sub","center_y",3.5],"center_z"],
        "max":[["add","center_x",20],["add","center_y",3.5],["add","center_z",15]]
    }"#,
    )
    .unwrap();
    let formula = raw.compile(&[]).unwrap();
    let envelope = Bounds {
        min: [-32, -64, -32],
        max: [0, 31, 0],
    };
    let inputs = Inputs([0.0, 0.0, 0.0, -16.0, -16.0, -16.0, 0.0, 0.0, 0.0, 0.0]);
    let clipped = envelope.intersect_formula(&formula, 42, inputs).unwrap();
    assert_eq!(clipped.min, [-32, -19, -16]);
    assert_eq!(clipped.max, [0, -13, -1]);
    assert!(clipped.intersects([-16, -16, -16], [-1, -1, -1]));
    assert!(!clipped.intersects([-16, 0, -16], [-1, 15, -1]));
    let mut bad: RawBounds = serde_json::from_str(r#"{"min":[0,0,0],"max":[1,1,1]}"#).unwrap();
    bad.min[1] = Expression::Name("moving_floor".into());
    assert!(bad
        .compile(&[("moving_floor".into(), Expression::Name("y".into()))])
        .is_err());
}
