//! The water trough: right-click with a water bucket to fill it, or with an
//! empty bucket to drain it. The held bucket swaps in place so it stays in
//! the player's hand.

use mod_sdk::*;

use crate::content::Content;

pub fn on_item_use(content: &Content, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };

    if block == content.trough && item == content.water_bucket {
        if !replace_held_one(content.water_bucket, "petramond:wooden_bucket") {
            return Outcome::Continue;
        }
        swap_model_block(pos, content.trough_filled);
        emit_sound("petramond:water_splash_small", Some(center(pos)));
        return Outcome::Cancel;
    }

    if block == content.trough_filled && item == content.wooden_bucket {
        if !replace_held_one(content.wooden_bucket, "petramond:water_bucket") {
            return Outcome::Continue;
        }
        swap_model_block(pos, content.trough);
        emit_sound("petramond:water_splash_small", Some(center(pos)));
        return Outcome::Cancel;
    }

    Outcome::Continue
}

fn center(pos: [i32; 3]) -> [f32; 3] {
    [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.5,
        pos[2] as f32 + 0.5,
    ]
}
