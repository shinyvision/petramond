use mod_sdk::*;
use weather_core::FieldParams;

use crate::content::Content;
use crate::farmland::{self, Hydration};
use crate::keys;

pub fn gate(
    content: &Content,
    world: &impl WorldView,
    item: ItemId,
    pos: [i32; 3],
    block: BlockId,
) -> Option<BlockId> {
    if item != content.iron_hoe {
        return None;
    }
    if block != content.grass && block != content.dirt && block != content.grass_fertilized {
        return None;
    }
    let cover = world.block([pos[0], pos[1] + 1, pos[2]])?;
    (cover == BlockId::AIR || content.is_clearable_cover(cover)).then_some(cover)
}

pub fn till(
    content: &Content,
    sky: Option<&FieldParams>,
    pos: [i32; 3],
    cover: BlockId,
) -> Outcome {
    let above = [pos[0], pos[1] + 1, pos[2]];
    if cover != BlockId::AIR {
        set_block(above, BlockId::AIR);
    }
    let soil = match farmland::probe(content, sky, pos) {
        Hydration::Hydrated => content.farmland_wet,
        Hydration::Dry | Hydration::Unknown => content.farmland_dry,
    };
    set_block(pos, soil);
    let center = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 1.0,
        pos[2] as f64 + 0.5,
    ];
    emit_sound(keys::TILL_SOUND, Some(center));
    emitter_burst(keys::TILL_BURST, center, 1.0);
    Outcome::Cancel
}
