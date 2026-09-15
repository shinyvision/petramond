use glam::Vec3;

use crate::bbmodel::{Animation, Channel, Interpolation, Keyframe, Marker, MarkerKind, Model};

/// root → leftArm → leftHand → leftFinger, root → rightArm → rightHand.
pub fn rig() -> Model {
    Model::load(
        r#"{
        "resolution": { "width": 16, "height": 16 },
        "textures": [{ "uv_width": 16, "uv_height": 16,
            "source": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" }],
        "elements": [],
        "groups": [
            { "uuid": "root", "name": "root", "origin": [0, 0, 0] },
            { "uuid": "la", "name": "leftArm", "origin": [5, 22, 0] },
            { "uuid": "lh", "name": "leftHand", "origin": [5, 12, 0] },
            { "uuid": "lf", "name": "leftFinger", "origin": [5, 10, 0] },
            { "uuid": "ra", "name": "rightArm", "origin": [-5, 22, 0] },
            { "uuid": "rh", "name": "rightHand", "origin": [-5, 12, 0] }
        ],
        "outliner": [{ "uuid": "root", "children": [
            { "uuid": "la", "children": [{ "uuid": "lh", "children": [{ "uuid": "lf", "children": [] }] }] },
            { "uuid": "ra", "children": [{ "uuid": "rh", "children": [] }] }
        ] }]
    }"#,
    )
    .expect("test rig parses")
}

/// A clip of linear rotation keys `(bone, time, degrees)`.
pub fn clip(rig: &Model, length: f32, looping: bool, keys: &[(&str, f32, Vec3)]) -> Animation {
    let mut anim = Animation::new(length, looping, false);
    let mut tracks: Vec<(usize, Vec<Keyframe>)> = Vec::new();
    for (bone, time, value) in keys {
        let bone = rig.bone_named(bone).expect("rig bone");
        let key = Keyframe::new(*time, *value, Interpolation::Linear);
        match tracks.iter_mut().find(|(b, _)| *b == bone) {
            Some((_, track)) => track.push(key),
            None => tracks.push((bone, vec![key])),
        }
    }
    for (bone, track) in tracks {
        anim.set_track(bone, Channel::Rotation, track);
    }
    anim
}

pub fn with_marker(mut anim: Animation, time: f32, name: &str) -> Animation {
    anim.push_marker(Marker {
        time,
        kind: MarkerKind::Timeline,
        name: name.to_string(),
    });
    anim
}
