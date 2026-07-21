//! Fertilizer application: ONE handler, a target table, one feedback path.
//!
//! What fertilizer does to a block is a row in [`target`]: plain farmland
//! upgrades to the fertile variant (hydration skin preserved — see
//! [`crate::compost`] for where fertilizer comes from), plain grass becomes
//! `farming:grass_fertilized` (see [`crate::spread`] for what that then
//! does), and a known engine sapling stage jumps to its species' FINAL stage
//! row (the tree still grows on the sapling's own later roll — "much
//! sooner", never "instantly"). Everything else — already-fertile soil, an
//! already-final sapling — falls through untouched: no wasted units, no
//! consumed click.
//!
//! A click on VEGETATION proxies to the soil beneath it: a crop fertilizes
//! its (unfertilized) farmland, and any other soil-rooted plant (flowers,
//! short grass, ferns — the spreadable set) fertilizes its grass block. The
//! same target table answers the soil cell, so fertile farmland and non-grass
//! soil still fall through untouched.
//!
//! [`apply`] owns the consume → swap → sound/burst → Cancel sequence for
//! every target, so the feedback keys can never drift apart per arm. The
//! SAPLING arm is the one flagged variant: stages are invisible, so the
//! boost happens BEFORE the consume (a refused write on a mid-stream cell
//! falls through with the unit kept, and an already-final sapling is
//! detected by block comparison above), and a failed consume still skips the
//! feedback and the Cancel like every other arm.

use mod_sdk::*;

use crate::content::Content;

/// What fertilizer would do to `block`, if anything.
enum Target {
    /// Consume one unit, then swap: farmland and grass.
    Swap { to: BlockId, feedback_y: f32 },
    /// Swap first (write-gated), then consume: the sapling boost.
    BoostFirst { to: BlockId, feedback_y: f32 },
}

/// The fertilizer target table.
fn target(content: &Content, block: BlockId) -> Option<Target> {
    if content.is_farmland(block) && !content.is_fertile(block) {
        let to = if block == content.farmland_wet {
            content.farmland_fertile_wet
        } else {
            content.farmland_fertile_dry
        };
        return Some(Target::Swap { to, feedback_y: 1.0 });
    }
    if block == content.grass {
        return Some(Target::Swap {
            to: content.grass_fertilized,
            feedback_y: 1.0,
        });
    }
    if let Some(last) = content.sapling_final(block) {
        // An already-final sapling: the boost would change nothing, so the
        // click falls through and no invisible-state fertilizer is wasted.
        if block != last {
            return Some(Target::BoostFirst {
                to: last,
                feedback_y: 0.5,
            });
        }
    }
    None
}

/// The fertilizer `item_use_pre` link: chained in lib.rs between the hoe and
/// the compost fill, falling through quietly when the held item or the
/// target is not its business.
pub fn on_item_use(content: &Content, item: ItemId, target_pos: Option<[i32; 3]>) -> Outcome {
    if item != content.fertilizer {
        return Outcome::Continue;
    }
    let Some(pos) = target_pos else {
        return Outcome::Continue;
    };
    // Unloaded / mid-stream reads mean "not actionable now" — quiet no-op.
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };
    let Some(action) = target(content, block) else {
        // A click on vegetation proxies to the soil beneath it: a crop
        // fertilizes its (unfertilized) farmland; any other soil-rooted plant
        // (flowers, short grass, ferns) fertilizes its grass block. Anything
        // else — fertile soil already, dirt, air — falls through quietly.
        let below = [pos[0], pos[1] - 1, pos[2]];
        let Some(soil) = get_block(below) else {
            return Outcome::Continue;
        };
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
        let Some(action) = action else {
            return Outcome::Continue;
        };
        return apply(below, item, action);
    };
    apply(pos, item, action)
}

/// The one apply sequence. Both arms gate the feedback + Cancel on the
/// consume; they differ only in whether the swap precedes it (the sapling's
/// pinned boost-before-consume order).
fn apply(pos: [i32; 3], item: ItemId, action: Target) -> Outcome {
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
            // Boost BEFORE consuming: an unloaded / mid-stream cell refuses
            // the write, the click falls through, and the unit is kept.
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
    let center = [pos[0] as f32 + 0.5, pos[1] as f32 + y, pos[2] as f32 + 0.5];
    emit_sound("farming:till", Some(center));
    emitter_burst("farming:fertilize_burst", center, 1.0);
}

/// CLIENT prediction mirror of [`on_item_use`]'s gate: the direct target
/// table, then the vegetation-to-soil proxy — the claim condition only,
/// never the swap.
pub fn predict_item_use(
    content: &Content,
    item: ItemId,
    pos: [i32; 3],
    block: BlockId,
) -> Outcome {
    if item != content.fertilizer {
        return Outcome::Continue;
    }
    if target(content, block).is_some() {
        return Outcome::Cancel;
    }
    let Some(soil) = crate::predict::peek([pos[0], pos[1] - 1, pos[2]]) else {
        return Outcome::Continue;
    };
    let claims = if content.crop_stage(block).is_some() {
        content.is_farmland(soil) && target(content, soil).is_some()
    } else {
        content.spreadable.contains(&block) && soil == content.grass
    };
    if claims {
        Outcome::Cancel
    } else {
        Outcome::Continue
    }
}
