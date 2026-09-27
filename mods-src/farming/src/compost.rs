use mod_sdk::*;

use crate::content::Content;
use crate::keys;

pub fn fill_gate(content: &Content, item: ItemId, block: BlockId) -> Option<u8> {
    if !content.compostable.contains(&item) {
        return None;
    }
    match content.compost_stage(block) {
        Some(stage @ 0..=2) => Some(stage),
        _ => None,
    }
}

pub fn fill(content: &Content, item: ItemId, pos: [i32; 3], stage: u8) -> Outcome {
    if !consume_held(item, 1) {
        return Outcome::Continue;
    }
    swap_block(pos, content.compost[stage as usize + 1]);
    let center = barrel_top(pos);
    emit_sound(keys::TILL_SOUND, Some(center));
    emitter_burst(keys::COMPOST_FILL, center, 1.0);
    Outcome::Cancel
}

pub fn collect_gate(content: &Content, block: BlockId) -> bool {
    content.compost_stage(block) == Some(3)
}

pub fn collect(content: &Content, pos: [i32; 3]) -> Outcome {
    let center = barrel_top(pos);
    spawn_item(keys::FERTILIZER, 1, center);
    swap_block(pos, content.compost[0]);
    emit_sound(keys::HARVEST_SOUND, Some(center));
    emitter_burst(keys::COMPOST_FILL, center, 1.0);
    Outcome::Cancel
}

fn barrel_top(pos: [i32; 3]) -> [f64; 3] {
    [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 1.2,
        pos[2] as f64 + 0.5,
    ]
}
