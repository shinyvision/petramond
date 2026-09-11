use super::*;

#[test]
fn disabling_directional_shading_keeps_occlusion_independent() {
    let appearance: SurfaceMaterial =
        serde_json::from_str(r#"{"rect":[0,0,1,1],"shade":false}"#).unwrap();
    assert!(!appearance.unlit);
    assert!(appearance.ambient_occlusion);
    let face = FaceAppearance {
        shade: appearance.shade,
        ..FaceAppearance::default()
    };
    assert_eq!(face.shading(0.5, 0.75), 0.75);
    assert_eq!(
        FaceAppearance {
            ambient_occlusion: false,
            ..face
        }
        .shading(0.5, 0.75),
        1.0
    );
    assert_eq!(
        FaceAppearance {
            unlit: true,
            ..FaceAppearance::default()
        }
        .shading(0.5, 0.75),
        1.0
    );
}
#[test]
fn appearance_keeps_animation_separate_from_albedo() {
    let p = FaceAppearance {
        tint: [128, 255, 64],
        animation: 37,
        ..FaceAppearance::default()
    };
    assert_eq!(p.packed(Some(0xff8040)), 0x25808010);
    assert_eq!(p.packed(None), 0x2580ff40);
}
#[test]
fn adjacent_regions_are_valid_but_overlap_is_ambiguous() {
    let a = SurfaceMaterial {
        rect: [0.0, 0.0, 1.0, 0.5],
        tint: white(),
        animation: None,
        unlit: false,
        shade: true,
        ambient_occlusion: true,
    };
    let b = SurfaceMaterial {
        rect: [0.0, 0.5, 1.0, 1.0],
        ..a
    };
    assert!(validate(&[a, b]).is_ok());
    assert!(validate(&[
        a,
        SurfaceMaterial {
            rect: [0.0, 0.4, 1.0, 0.8],
            ..b
        }
    ])
    .is_err());
}
