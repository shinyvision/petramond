use super::*;
use crate::mob::{damage::DeathState, model_meta, Mob, MobDamageFeedback};
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

#[test]
fn death_freezes_the_live_pose_before_feedback_and_keeps_it_through_physics_init() {
    let mut mob = Instance::new(Mob::Owl, WorldPos::ZERO, 0.0, 7);
    mob.moving = true;
    mob.anim_time = 0.17;
    mob.head_yaw = 0.6;
    mob.head_pitch = 0.3;
    let expected = mob.death_pose();
    assert!(expected
        .iter()
        .any(|m| !m.abs_diff_eq(Mat4::IDENTITY, 1e-3)));
    assert!(mob.damage(100.0, None, false, None, &MobDamageFeedback::default()));
    let before_init = mob.ragdoll_pose(1.0).expect("immediate corpse pose");
    let model = model(mob.kind);
    for (i, (bone, rest)) in model.bones.iter().zip(model.rest_pose()).enumerate() {
        let pivot = expected[i].transform_point3(rest.transform_point3(bone.pivot));
        assert!((before_init[i].0 - pivot).length() < 1e-3);
        assert!(
            before_init[i]
                .1
                .angle_between(expected[i].to_scale_rotation_translation().1)
                < 1e-3
        );
    }
    let DeathState::Ragdoll(rag) = &mut mob.combat.death else {
        panic!("ragdoll death")
    };
    rag.init(
        &model_meta::skeleton(model),
        crate::mob::def(mob.kind).scale,
        Vec3::ZERO,
        0.0,
    );
    for alpha in [0.0, 0.5, 1.0] {
        for ((p, q), (expected_p, expected_q)) in rag.pose(alpha).into_iter().zip(&before_init) {
            assert!((p - expected_p).length() < 1e-3);
            assert!(q.angle_between(*expected_q) < 1e-3);
        }
    }
}
