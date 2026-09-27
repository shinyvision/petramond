use mod_sdk::*;

use crate::content::Content;
use crate::keys;

#[derive(Copy, Clone)]
pub enum Target {
    Swap { to: BlockId, feedback_y: f32 },
    BoostFirst { to: BlockId, feedback_y: f32 },
}

fn target(content: &Content, block: BlockId) -> Option<Target> {
    if content.is_farmland(block) && !content.is_fertile(block) {
        let to = if block == content.farmland_wet {
            content.farmland_fertile_wet
        } else {
            content.farmland_fertile_dry
        };
        return Some(Target::Swap {
            to,
            feedback_y: 1.0,
        });
    }
    if block == content.grass {
        return Some(Target::Swap {
            to: content.grass_fertilized,
            feedback_y: 1.0,
        });
    }
    if let Some(last) = content.sapling_final(block) {
        if block != last {
            return Some(Target::BoostFirst {
                to: last,
                feedback_y: 0.5,
            });
        }
    }
    None
}

pub fn gate(
    content: &Content,
    world: &impl WorldView,
    item: ItemId,
    pos: [i32; 3],
    block: BlockId,
) -> Option<([i32; 3], Target)> {
    if item != content.fertilizer {
        return None;
    }
    if let Some(action) = target(content, block) {
        return Some((pos, action));
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    let soil = world.block(below)?;
    let action = if content.crop_stage(block).is_some() {
        if content.is_farmland(soil) {
            target(content, soil)
        } else {
            None
        }
    } else if content.spreadable.contains(&block) && soil == content.grass {
        Some(Target::Swap {
            to: content.grass_fertilized,
            feedback_y: 1.0,
        })
    } else {
        None
    };
    action.map(|action| (below, action))
}

pub fn apply(pos: [i32; 3], item: ItemId, action: Target) -> Outcome {
    match action {
        Target::Swap { to, feedback_y } => {
            if !consume_held(item, 1) {
                return Outcome::Continue;
            }
            set_block(pos, to);
            feedback(pos, feedback_y);
            Outcome::Cancel
        }
        Target::BoostFirst { to, feedback_y } => {
            if !set_block(pos, to) {
                return Outcome::Continue;
            }
            if !consume_held(item, 1) {
                return Outcome::Continue;
            }
            feedback(pos, feedback_y);
            Outcome::Cancel
        }
    }
}

fn feedback(pos: [i32; 3], y: f32) {
    let center = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + f64::from(y),
        pos[2] as f64 + 0.5,
    ];
    emit_sound(keys::TILL_SOUND, Some(center));
    emitter_burst(keys::FERTILIZE_BURST, center, 1.0);
}
