use super::*;
use crate::skinned::skin_positions;
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
        gait_fades: crate::ArenaRange::default(),
        head_yaw: 0.0,
        head_pitch: 0.0,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        hurt: 0.0,
        shorn: false,
        emitter_tint: [1.0, 1.0, 1.0],
        emitter_self_lit: 0.0,
        anims: crate::ArenaRange::default(),
        ragdoll: None,
        held: [None; 2],
    }
}

/// Pose `insts` of one species into a fresh batch, their ranges addressing
/// `layers`.
fn pose_with(
    model: &Model,
    scale: f32,
    insts: &[MobRenderInstance],
    layers: MobLayers<'_>,
    rig: &MobRig,
) -> SkinBatch {
    let mut batch = SkinBatch::default();
    let range = pose_mob_instances(
        model,
        scale,
        insts,
        layers,
        petramond_math::math::IVec3::ZERO,
        rig,
        &mut MobPoseCache::default(),
        &mut batch,
        &mut Vec::new(),
    );
    assert_eq!(range, 0..insts.len() as u32);
    batch
}

/// Pose `insts` of one species, none of which addresses an arena row.
fn pose(model: &Model, scale: f32, insts: &[MobRenderInstance], rig: &MobRig) -> SkinBatch {
    let layers = MobLayers {
        arena: &crate::MobArena::default(),
        names: &crate::AnimNames::default(),
    };
    pose_with(model, scale, insts, layers, rig)
}

/// Pose one instance and skin it: the render-local positions the GPU draws.
fn skin(model: &Model, scale: f32, inst: &MobRenderInstance, rig: &MobRig) -> Vec<Vec3> {
    let batch = pose(model, scale, std::slice::from_ref(inst), rig);
    skin_positions(&rig.mesh(model, scale), &batch, 0)
}

/// Skin instance `i` of `insts`, posed against `layers`.
fn skin_with(
    model: &Model,
    insts: &[MobRenderInstance],
    layers: MobLayers<'_>,
    rig: &MobRig,
    i: u32,
) -> Vec<Vec3> {
    let batch = pose_with(model, 0.25, insts, layers, rig);
    skin_positions(&rig.mesh(model, 0.25), &batch, i)
}

#[test]
fn empty_instances_produce_no_instances() {
    let m = owl_model();
    let batch = pose(&m, 0.25, &[], &MobRig::resolve(&m, None, None));
    assert!(batch.instances.is_empty() && batch.palette.is_empty());
}

#[test]
fn each_instance_gets_its_own_palette_run() {
    let m = owl_model();
    let rig = MobRig::resolve(&m, None, None);
    let batch = pose(&m, 0.25, &[instance(0.0, true), instance(0.3, true)], &rig);
    let slots = bone_slots(&m);
    assert_eq!(batch.palette.len() as u32, 2 * slots);
    assert_eq!(batch.instances[0].bone_base, 0);
    assert_eq!(batch.instances[1].bone_base, slots);
    // Species draw ranges follow on from whatever is already in the batch.
    let mut batch = batch;
    let range = pose_mob_instances(
        &m,
        0.25,
        std::slice::from_ref(&instance(0.0, false)),
        MobLayers {
            arena: &crate::MobArena::default(),
            names: &crate::AnimNames::default(),
        },
        petramond_math::math::IVec3::ZERO,
        &rig,
        &mut MobPoseCache::default(),
        &mut batch,
        &mut Vec::new(),
    );
    assert_eq!(range, 2..3);
}

#[test]
fn every_pose_carries_its_look_to_the_instance() {
    // Live and ragdolling mobs alike hand the shader their hurt flash, fire
    // tint, self-lighting and sampled light — the inputs the CPU bake folded
    // into every vertex's tint (`skinned` pins the shader's fold against
    // `body_tint`).
    let model = owl_model();
    let rest = model.rest_pose();
    let arena = crate::MobArena {
        ragdoll: model
            .bones
            .iter()
            .enumerate()
            .map(|(b, bone)| {
                (
                    rest[b].transform_point3(bone.pivot),
                    glam::Quat::from_rotation_z(0.6),
                )
            })
            .collect(),
        ..Default::default()
    };
    let names = crate::AnimNames::default();
    let bones = crate::ArenaRange::since(&arena.ragdoll, 0);
    for ragdoll in [None, Some(bones)] {
        let mut inst = instance(0.0, false);
        inst.ragdoll = ragdoll;
        inst.emitter_tint = [1.0, 0.7, 0.4];
        inst.emitter_self_lit = 1.0;
        inst.hurt = 0.5;
        inst.skylight = 21;
        inst.blocklight = petramond_world::light::BlockLight6::new(40, 0, 63);
        let batch = pose_with(
            &model,
            0.25,
            std::slice::from_ref(&inst),
            MobLayers {
                arena: &arena,
                names: &names,
            },
            &MobRig::resolve(&model, None, None),
        );
        let row = batch.instances[0];
        assert_eq!(row.tint, mul3(hurt_tint(0.5), [1.0, 0.7, 0.4]));
        assert_eq!(row.self_lit, 1.0);
        assert_eq!(row.light[0], 21.0 / 63.0);
        assert_eq!(&row.light[1..], &inst.blocklight.fractions());
        assert_eq!(row.hidden, 0);
    }
}

#[test]
fn the_species_mesh_is_whole_quads_with_matched_indices() {
    let m = owl_model();
    let mesh = MobRig::resolve(&m, None, None).mesh(&m, 0.25);
    assert!(mesh.index_count() > 0);
    assert_eq!(mesh.verts.len() % 4, 0);
    assert_eq!(mesh.verts.len() / 4 * 6, mesh.indices.len());
    assert!(mesh.indices.iter().all(|&ix| (ix as usize) < mesh.verts.len()));
    assert!(mesh.verts.iter().all(|v| v.bone < bone_slots(&m)));
}

#[test]
fn scale_sizes_the_posed_model() {
    let m = owl_model();
    let rig = MobRig::resolve(&m, None, None);
    let inst = instance(0.0, false);
    let v1 = skin(&m, 0.25, &inst, &rig);
    let v2 = skin(&m, 0.5, &inst, &rig);
    // Same geometry, double scale -> double the vertical extent above the feet.
    let span = |v: &[Vec3]| v.iter().map(|x| x.y).fold(f32::MIN, f32::max) - 64.0;
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
    let rig = MobRig::resolve(&m, None, None);
    let at = |t: f32, moving: bool| skin(&m, 0.25, &instance(t, moving), &rig);
    let walk_a = at(0.0, true);
    let walk_b = at(0.25, true);
    let moved = walk_a
        .iter()
        .zip(&walk_b)
        .any(|(a, b)| (a.z - b.z).abs() > 1e-3);
    assert!(moved, "walking mob's legs move between phases");

    let rest_a = at(0.0, false);
    let rest_b = at(0.25, false);
    assert!(
        rest_a == rest_b,
        "idle mob ignores anim_time (always the rest pose)"
    );
}

#[test]
fn shorn_hides_exactly_the_wool_named_cubes() {
    // A shorn sheep draws without its `wool` cubes; a model with no wool-named
    // cubes (the owl) draws identically shorn or not — proving the hiding keys
    // on the cubes the rig names as coat, not on the shorn flag alone.
    let sheep = Model::load(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/models/sheep.bbmodel"
    )))
    .expect("sheep model");
    assert!(
        sheep.cubes.iter().any(|c| c.name == "wool"),
        "fixture must author its fleece as `wool` cubes"
    );
    let draw = |model: &Model, kind: Mob, shorn: bool| {
        let mut inst = instance(0.0, false);
        inst.kind = kind;
        inst.shorn = shorn;
        skin(
            model,
            0.0625,
            &inst,
            &MobRig::resolve(model, None, Some("wool")),
        )
    };
    let coated = draw(&sheep, Mob::Sheep, false);
    let shorn = draw(&sheep, Mob::Sheep, true);
    assert!(
        shorn.len() < coated.len(),
        "hiding the fleece removes geometry: {} -> {}",
        coated.len(),
        shorn.len()
    );

    let owl = owl_model();
    assert!(owl.cubes.iter().all(|c| c.name != "wool"));
    let owl_plain = draw(&owl, Mob::Owl, false);
    let owl_shorn = draw(&owl, Mob::Owl, true);
    assert_eq!(
        owl_plain.len(),
        owl_shorn.len(),
        "a model without wool cubes is unaffected by shorn"
    );
}

#[test]
fn head_look_rotates_the_head_when_idle() {
    // Idle (rest pose, no head animation): a non-zero head_yaw must move the head
    // cubes, confirming head-look is wired into the pose.
    let m = owl_model();
    let rig = MobRig::resolve(&m, None, None);
    let looking = |head_yaw: f32| {
        let mut inst = instance(0.0, false);
        inst.head_yaw = head_yaw;
        skin(&m, 0.25, &inst, &rig)
    };
    let straight = looking(0.0);
    let turned = looking(1.0);
    assert!(
        straight.iter().zip(&turned).any(|(a, b)| a != b),
        "head-look should rotate the head when idle"
    );
}

/// Each instance's arena ranges address only its own rows: a named layer
/// (an interned id naming the walk clip) and a fading gait pose their own
/// body exactly as the base walk would, an unknown name poses nothing, and a
/// neighbour with no rows in the arena stays at rest.
#[test]
fn arena_ranges_pose_each_instance_from_its_own_layers() {
    let m = owl_model();
    let rig = MobRig::resolve(&m, None, None);
    let mut interner = crate::AnimInterner::default();
    let walk = interner.intern(clips::WALK);
    let missing = interner.intern("no_such_clip");
    assert_eq!(interner.intern(clips::WALK), walk, "a name interns once");
    let arena = crate::MobArena {
        gait_fades: vec![crate::GaitFade {
            clip: crate::GaitClip::Walk,
            phase: 0.25,
            weight: 1.0,
        }],
        anims: vec![
            crate::AnimLayer {
                anim: missing,
                phase: 0.25,
                weight: 1.0,
            },
            crate::AnimLayer {
                anim: walk,
                phase: 0.25,
                weight: 1.0,
            },
        ],
        ragdoll: Vec::new(),
    };
    let layers = MobLayers {
        arena: &arena,
        names: interner.names(),
    };
    let mut named = instance(0.0, false);
    named.anims = crate::ArenaRange { start: 0, len: 2 };
    let mut fading = instance(0.0, false);
    fading.gait_fades = crate::ArenaRange { start: 0, len: 1 };
    let mut unknown = instance(0.0, false);
    unknown.anims = crate::ArenaRange { start: 0, len: 1 };
    let insts = [named, fading, unknown, instance(0.0, false)];

    let walking = skin(&m, 0.25, &instance(0.25, true), &rig);
    let rest = skin(&m, 0.25, &instance(0.0, false), &rig);
    assert_eq!(skin_with(&m, &insts, layers, &rig, 0), walking);
    assert_eq!(skin_with(&m, &insts, layers, &rig, 1), walking);
    assert_eq!(skin_with(&m, &insts, layers, &rig, 2), rest);
    assert_eq!(skin_with(&m, &insts, layers, &rig, 3), rest);
}

/// A resolved id is reused while the table still names it the same way, and
/// resolves afresh against another session's table that reuses the id for a
/// different name.
#[test]
fn anim_clips_follow_the_table_that_names_the_id() {
    let m = owl_model();
    let mut ours = crate::AnimInterner::default();
    let mut theirs = crate::AnimInterner::default();
    let id = ours.intern(clips::WALK);
    assert_eq!(theirs.intern("no_such_clip"), id, "both tables start at 0");
    fn ptr(clip: Option<&Animation>) -> Option<*const Animation> {
        clip.map(|a| a as *const Animation)
    }
    let walk = ptr(m.animation(clips::WALK));
    assert!(walk.is_some(), "the fixture has a walk clip");
    let mut cache = AnimClips::default();
    assert_eq!(ptr(cache.resolve(&m, ours.names(), id)), walk);
    assert_eq!(ptr(cache.resolve(&m, ours.names(), id)), walk);
    assert_eq!(ptr(cache.resolve(&m, theirs.names(), id)), None);
    assert_eq!(ptr(cache.resolve(&m, ours.names(), id)), walk);
    assert!(
        cache
            .resolve(&m, ours.names(), crate::AnimId(9))
            .is_none(),
        "an id outside the table resolves to nothing"
    );
}
