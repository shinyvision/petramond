//! The eased held-item state: a hand's per-frame [`HeldItemFrame`] intent
//! becomes the flat [`HeldItemView`] the seats and attaches read — the item,
//! its authored hold, and the claimed held pose CHASED at the pose rate so a
//! replicated publisher's 20 Hz stair-steps read as a glide. The renderer
//! owns one per local hand; every remote player owns two (`game/remote_players.rs`).

use petramond_world::item::ItemType;

use super::{HeldItemFrame, HeldItemView, HeldPose, POSE_EASE_RATE};

#[derive(Copy, Clone, Debug, Default)]
pub struct HeldItemEase {
    pose: HeldPose,
    /// Which item the eased pose belongs to. A pose is state ABOUT AN ITEM,
    /// so a hand that changed item must not glide the old item's offset onto
    /// the new one.
    posed_item: Option<ItemType>,
}

impl HeldItemEase {
    /// This hand's view for `frame`, `dt` seconds after the last.
    pub fn update(&mut self, frame: &HeldItemFrame, dt: f32) -> HeldItemView {
        if frame.item != self.posed_item {
            self.posed_item = frame.item;
            self.pose = HeldPose::default();
        }
        // `None` eases back to the item's authored hold.
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
