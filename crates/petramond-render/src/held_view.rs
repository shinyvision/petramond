use petramond_world::item::ItemType;

use super::{HeldItemFrame, HeldItemView, HeldPose, POSE_EASE_RATE};

#[derive(Copy, Clone, Debug, Default)]
pub struct HeldItemEase {
    pose: HeldPose,
    posed_item: Option<ItemType>,
}

impl HeldItemEase {
    pub fn update(&mut self, frame: &HeldItemFrame, dt: f32) -> HeldItemView {
        if frame.item != self.posed_item {
            self.posed_item = frame.item;
            self.pose = HeldPose::default();
        }
        self.pose.ease_toward(
            &frame.pose_target.unwrap_or_default(),
            1.0 - (-POSE_EASE_RATE * dt.max(0.0)).exp(),
        );
        HeldItemView {
            item: frame.display.or(frame.item),
            hold: frame
                .item
                .map(|item| item.held_pose())
                .unwrap_or(petramond_world::item::HeldPose::DEFAULT),
            variant: frame.variant,
            block_state: frame.block_state,
            pose: self.pose,
        }
    }
}
