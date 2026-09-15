use glam::{Mat4, Vec3};

use super::{swap_side, LocalPose, MirrorMap};
use crate::bbmodel::Model;

/// A left/right symmetric rig whose arms carry mirrored rest rotations, with a
/// clip that turns the body and the left arm about all three axes at once.
fn rig() -> Model {
    Model::load(
        r#"{
        "resolution": { "width": 16, "height": 16 },
        "textures": [{ "uv_width": 16, "uv_height": 16,
            "source": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" }],
        "elements": [],
        "groups": [
            { "uuid": "b", "name": "body", "origin": [0, 12, 0] },
            { "uuid": "la", "name": "leftArm", "origin": [5, 22, 0], "rotation": [0, 10, 15] },
            { "uuid": "le", "name": "left_elbow", "origin": [6, 16, 1] },
            { "uuid": "ra", "name": "rightArm", "origin": [-5, 22, 0], "rotation": [0, -10, -15] },
            { "uuid": "re", "name": "right_elbow", "origin": [-6, 16, 1] },
            { "uuid": "h", "name": "head", "origin": [0, 24, 0] }
        ],
        "outliner": [{ "uuid": "b", "children": [
            { "uuid": "la", "children": [{ "uuid": "le", "children": [] }] },
            { "uuid": "ra", "children": [{ "uuid": "re", "children": [] }] },
            { "uuid": "h", "children": [] }
        ] }],
        "animations": [{
            "name": "reach", "loop": "loop", "length": 1.0,
            "animators": {
                "la": { "name": "leftArm", "type": "bone", "keyframes": [
                    { "channel": "rotation", "time": 0, "interpolation": "catmullrom",
                      "data_points": [{ "x": "0", "y": "0", "z": "0" }] },
                    { "channel": "rotation", "time": 0.5, "interpolation": "catmullrom",
                      "data_points": [{ "x": "-60", "y": "20", "z": "-35" }] },
                    { "channel": "position", "time": 0.5,
                      "data_points": [{ "x": "1", "y": "-2", "z": "3" }] }
                ] },
                "le": { "name": "left_elbow", "type": "bone", "keyframes": [
                    { "channel": "rotation", "time": 0,
                      "data_points": [{ "x": "-45", "y": "12", "z": "8" }] }
                ] },
                "b": { "name": "body", "type": "bone", "keyframes": [
                    { "channel": "rotation", "time": 0,
                      "data_points": [{ "x": "5", "y": "25", "z": "-10" }] },
                    { "channel": "position", "time": 0,
                      "data_points": [{ "x": "0.5", "y": "0", "z": "-1" }] }
                ] }
            }
        }]
    }"#,
    )
    .expect("rig parses")
}

#[test]
fn a_summed_pose_resolves_exactly_as_the_clip_path_does() {
    let m = rig();
    let reach = m.animation("reach").expect("reach");
    for t in [0.0, 0.3, 0.5, 0.85] {
        let mut pose = LocalPose::rest(m.bones().len());
        pose.add_clip(reach, t, 1.0);
        for (a, b) in pose.resolve(&m).iter().zip(m.pose(reach, t)) {
            assert!(a.abs_diff_eq(b, 1e-5), "t={t}: {a} vs {b}");
        }
    }
}

#[test]
fn a_mirrored_pose_is_the_reflection_of_the_original_across_the_rig() {
    let m = rig();
    let map = MirrorMap::for_model(&m);
    let bone = |n: &str| m.bone_named(n).expect(n);
    assert_eq!(map.partner(bone("leftArm")), bone("rightArm"));
    assert_eq!(map.partner(bone("right_elbow")), bone("left_elbow"));
    assert_eq!(map.partner(bone("head")), bone("head"));

    let mut pose = LocalPose::rest(m.bones().len());
    pose.add_clip(m.animation("reach").expect("reach"), 0.4, 1.0);
    let mut mirrored = LocalPose::rest(0);
    pose.mirror_into(&map, &mut mirrored);

    let flip = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let original = pose.resolve(&m);
    for (i, got) in mirrored.resolve(&m).iter().enumerate() {
        let want = flip * original[map.partner(i)] * flip;
        assert!(
            got.abs_diff_eq(want, 1e-4),
            "{}: {got} vs reflected {want}",
            m.bones()[i].name
        );
    }
}

#[test]
fn side_names_swap_whatever_their_spelling() {
    assert_eq!(swap_side("leftArm").as_deref(), Some("rightArm"));
    assert_eq!(swap_side("right_shoulder").as_deref(), Some("left_shoulder"));
    assert_eq!(swap_side("LeftLeg").as_deref(), Some("RightLeg"));
    assert_eq!(swap_side("item_right").as_deref(), Some("item_left"));
    assert_eq!(swap_side("head"), None);
}

#[test]
fn a_masked_blend_moves_only_the_bones_inside_the_mask() {
    let mut base = LocalPose::rest(3);
    let mut target = LocalPose::rest(3);
    for b in 0..3 {
        target.add_bone(b, Vec3::splat(10.0), Vec3::splat(2.0));
    }
    base.blend_toward(&target, 0.5, Some(&[1.0, 0.5]));
    assert_eq!(base.rotation(0), Vec3::splat(5.0));
    assert_eq!(base.rotation(1), Vec3::splat(2.5));
    assert_eq!(base.position(1), Vec3::splat(0.5));
    assert_eq!(base.rotation(2), Vec3::ZERO, "a bone past the mask is outside it");
}
