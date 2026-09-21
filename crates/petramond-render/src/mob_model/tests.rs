use super::*;
use petramond::mob::Mob;
use petramond_math::world_pos::WorldPos;

fn owl_model() -> Model {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/models/owl.bbmodel"
    ));
    Model::load(src).expect("owl model")
}

fn instance(anim_time: f32, moving: bool) -> MobRenderInstance {
    MobRenderInstance {
        kind: Mob::Owl,
        pos: WorldPos::new(10.0, 64.0, -5.0),
        yaw: 0.0,
        tilt: petramond_math::math::Tilt::LEVEL,
        anim_time,
        moving,
        idle_anim: None,
        gait_weight: 1.0,
        gait_fades: Vec::new(),
        head_yaw: 0.0,
        head_pitch: 0.0,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        hurt: 0.0,
        shorn: false,
        emitter_tint: [1.0, 1.0, 1.0],
        emitter_self_lit: 0.0,
        anims: Vec::new(),
        ragdoll: None,
        held: [None; 2],
    }
}

#[test]
fn empty_instances_produce_no_geometry() {
    let m = owl_model();
    let mut v = Vec::new();
    let mut i = Vec::new();
    assert_eq!(
        build_mob_instances(
            &m,
            0.25,
            LightEnv::IDENTITY,
            &[],
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
            &mut Vec::new(),
            &MobRig::resolve(&m, None, None),
        ),
        0
    );
    assert!(v.is_empty() && i.is_empty());
}

#[test]
fn self_lit_mobs_and_ragdolls_keep_their_tints_in_darkness() {
    let model = owl_model();
    let bake = |inst: &MobRenderInstance, env| {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_mob_instances(
            &model,
            0.25,
            env,
            std::slice::from_ref(inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
            &mut Vec::new(),
            &MobRig::resolve(&model, None, None),
        );
        assert!(!v.is_empty());
        v
    };
    let rest = model.rest_pose();
    let ragdoll = model
        .bones
        .iter()
        .enumerate()
        .map(|(b, bone)| {
            (
                rest[b].transform_point3(bone.pivot),
                glam::Quat::from_rotation_z(0.6),
            )
        })
        .collect::<Vec<_>>()
        .into();
    let night = LightEnv {
        sky_scale: 0.0,
        sky_color: [0.6, 0.7, 1.0],
    };
    for pose in [None, Some(ragdoll)] {
        let mut inst = instance(0.0, false);
        inst.ragdoll = pose;
        inst.emitter_tint = [1.0, 0.7, 0.4];
        inst.hurt = 0.5;
        let daylight = bake(&inst, LightEnv::IDENTITY);
        for sky in [0, 63] {
            inst.skylight = sky;
            inst.emitter_self_lit = 0.0;
            let dark = bake(&inst, night);
            inst.emitter_self_lit = 1.0;
            let lit = bake(&inst, night);
            for ((day, lit), dark) in daylight.iter().zip(&lit).zip(&dark) {
                assert_eq!(
                    lit.tint, day.tint,
                    "body light preserves fire tint and hurt flash"
                );
                assert_eq!(lit.shade, day.shade);
                assert!(
                    dark.tint[0] < lit.tint[0],
                    "ordinary bodies still follow world light"
                );
            }
        }
    }
}

#[test]
fn one_mob_bakes_quads_with_matched_indices() {
    let m = owl_model();
    let mut v = Vec::new();
    let mut i = Vec::new();
    let n = build_mob_instances(
        &m,
        0.25,
        LightEnv::IDENTITY,
        std::slice::from_ref(&instance(0.0, true)),
        petramond_math::math::IVec3::ZERO,
        &mut v,
        &mut i,
        &mut Vec::new(),
        &MobRig::resolve(&m, None, None),
    );
    assert!(n > 0);
    assert_eq!(v.len() % 4, 0);
    assert_eq!(n as usize, i.len());
    assert_eq!(v.len() / 4 * 6, i.len());
    assert!(i.iter().all(|&ix| (ix as usize) < v.len()));
}

#[test]
fn scale_sizes_the_baked_model() {
    let m = owl_model();
    let (mut v1, mut i1) = (Vec::new(), Vec::new());
    let (mut v2, mut i2) = (Vec::new(), Vec::new());
    build_mob_instances(
        &m,
        0.25,
        LightEnv::IDENTITY,
        std::slice::from_ref(&instance(0.0, false)),
        petramond_math::math::IVec3::ZERO,
        &mut v1,
        &mut i1,
        &mut Vec::new(),
        &MobRig::resolve(&m, None, None),
    );
    build_mob_instances(
        &m,
        0.5,
        LightEnv::IDENTITY,
        std::slice::from_ref(&instance(0.0, false)),
        petramond_math::math::IVec3::ZERO,
        &mut v2,
        &mut i2,
        &mut Vec::new(),
        &MobRig::resolve(&m, None, None),
    );
    // Same geometry, double scale -> double the vertical extent above the feet.
    let span = |v: &[ItemVertex]| v.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max) - 64.0;
    let (s1, s2) = (span(&v1), span(&v2));
    assert!(
        (s2 - s1 * 2.0).abs() < 1e-3,
        "scale should size the model: {s1} vs {s2}"
    );
}

#[test]
fn moving_plays_walk_idle_uses_rest_pose() {
    // A moving mob at two phases differs (legs swing); two idle mobs are identical
    // regardless of anim_time (both render the rest pose).
    let m = owl_model();
    let bake = |t: f32, moving: bool| {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_mob_instances(
            &m,
            0.25,
            LightEnv::IDENTITY,
            std::slice::from_ref(&instance(t, moving)),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
            &mut Vec::new(),
            &MobRig::resolve(&m, None, None),
        );
        v
    };
    let walk_a = bake(0.0, true);
    let walk_b = bake(0.25, true);
    let moved = walk_a
        .iter()
        .zip(&walk_b)
        .any(|(a, b)| (a.pos[2] - b.pos[2]).abs() > 1e-3);
    assert!(moved, "walking mob's legs move between phases");

    let rest_a = bake(0.0, false);
    let rest_b = bake(0.25, false);
    assert!(
        rest_a.iter().zip(&rest_b).all(|(a, b)| a.pos == b.pos),
        "idle mob ignores anim_time (always the rest pose)"
    );
}

#[test]
fn shorn_hides_exactly_the_wool_named_cubes() {
    // A shorn sheep bakes without its `wool` cubes; a model with no wool-named
    // cubes (the owl) bakes identically shorn or not — proving the skip keys on
    // the cubes the rig names as coat, not on the shorn flag alone.
    let sheep = Model::load(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/models/sheep.bbmodel"
    )))
    .expect("sheep model");
    assert!(
        sheep.cubes.iter().any(|c| c.name == "wool"),
        "fixture must author its fleece as `wool` cubes"
    );
    let bake = |model: &Model, kind: Mob, shorn: bool| {
        let mut inst = instance(0.0, false);
        inst.kind = kind;
        inst.shorn = shorn;
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_mob_instances(
            model,
            0.0625,
            LightEnv::IDENTITY,
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
            &mut Vec::new(),
            &MobRig::resolve(model, None, Some("wool")),
        );
        v
    };
    let coated = bake(&sheep, Mob::Sheep, false);
    let shorn = bake(&sheep, Mob::Sheep, true);
    assert!(
        shorn.len() < coated.len(),
        "hiding the fleece removes geometry: {} -> {}",
        coated.len(),
        shorn.len()
    );

    let owl = owl_model();
    assert!(owl.cubes.iter().all(|c| c.name != "wool"));
    let owl_plain = bake(&owl, Mob::Owl, false);
    let owl_shorn = bake(&owl, Mob::Owl, true);
    assert_eq!(
        owl_plain.len(),
        owl_shorn.len(),
        "a model without wool cubes is unaffected by shorn"
    );
}

#[test]
fn head_look_rotates_the_head_when_idle() {
    // Idle (rest pose, no head animation): a non-zero head_yaw must move the head
    // cubes, confirming head-look is wired into the bake.
    let m = owl_model();
    let bake = |head_yaw: f32| {
        let mut inst = instance(0.0, false);
        inst.head_yaw = head_yaw;
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_mob_instances(
            &m,
            0.25,
            LightEnv::IDENTITY,
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
            &mut Vec::new(),
            &MobRig::resolve(&m, None, None),
        );
        v
    };
    let straight = bake(0.0);
    let turned = bake(1.0);
    assert!(
        straight.iter().zip(&turned).any(|(a, b)| a.pos != b.pos),
        "head-look should rotate the head when idle"
    );
}
