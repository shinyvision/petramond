use glam::Mat4;

use super::FirstPersonHand;

#[test]
fn the_shipped_viewmodel_rig_loads_at_rest() {
    let hand = FirstPersonHand::shipped().expect("the viewmodel rig");
    assert_eq!(hand.bones.len(), hand.rig.row.model.bones().len());
}

#[test]
fn the_hand_takes_the_clients_pose_and_refuses_a_foreign_one() {
    let mut hand = FirstPersonHand::shipped().expect("the viewmodel rig");
    let posed: Vec<Mat4> = hand
        .bones
        .iter()
        .map(|bone| Mat4::from_rotation_x(0.3) * *bone)
        .collect();
    hand.set_bones(&posed);
    assert_eq!(hand.bones, posed);
    hand.set_bones(&posed[1..]);
    assert_eq!(hand.bones, posed, "a short pose is not this rig's");
    hand.reset();
    assert_eq!(hand.bones, hand.rig.row.model.resolve_local(&[], &[]));
}
