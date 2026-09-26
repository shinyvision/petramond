//! Every pack id this mod names, declared once and checked against the
//! shipped pack data by the test below: this pack's rows, the engine rows
//! the logic reads, sounds, bursts, the effect, a recipe, tags, the
//! husbandry data key, the block hooks and the brain nodes.
//!
//! Keys that are not pack declarations — the per-mob husbandry/hop/follow
//! tags written at runtime, cell-KV counters, world-KV rest keys, rng
//! streams — stay beside the code that owns them.

mod_sdk::pack_keys! {
    // --- this pack's blocks ---------------------------------------------
    pub FARMLAND_DRY: Block = "farming:farmland_dry";
    pub FARMLAND_WET: Block = "farming:farmland_wet";
    pub FARMLAND_FERTILE_DRY: Block = "farming:farmland_fertile_dry";
    pub FARMLAND_FERTILE_WET: Block = "farming:farmland_fertile_wet";
    pub WILD_WHEAT: Block = "farming:wild_wheat";
    pub WILD_CARROTS: Block = "farming:wild_carrots";
    pub WILD_POTATOES: Block = "farming:wild_potatoes";
    pub WHEAT_0: Block = "farming:wheat_0";
    pub WHEAT_1: Block = "farming:wheat_1";
    pub WHEAT_2: Block = "farming:wheat_2";
    pub WHEAT_3: Block = "farming:wheat_3";
    pub CARROTS_0: Block = "farming:carrots_0";
    pub CARROTS_1: Block = "farming:carrots_1";
    pub CARROTS_2: Block = "farming:carrots_2";
    pub CARROTS_3: Block = "farming:carrots_3";
    pub POTATOES_0: Block = "farming:potatoes_0";
    pub POTATOES_1: Block = "farming:potatoes_1";
    pub POTATOES_2: Block = "farming:potatoes_2";
    pub POTATOES_3: Block = "farming:potatoes_3";
    pub HEMP_0: Block = "farming:hemp_0";
    pub HEMP_1: Block = "farming:hemp_1";
    pub HEMP_2: Block = "farming:hemp_2";
    pub HEMP_3: Block = "farming:hemp_3";
    pub COMPOST_0: Block = "farming:compost_0";
    pub COMPOST_1: Block = "farming:compost_1";
    pub COMPOST_2: Block = "farming:compost_2";
    pub COMPOST_3: Block = "farming:compost_3";
    pub TROUGH: Block = "farming:trough";
    pub TROUGH_FILLED: Block = "farming:trough_filled";
    pub TROUGH_WHEAT: Block = "farming:trough_wheat";
    pub GRASS_FERTILIZED: Block = "farming:grass_fertilized";

    // --- engine blocks the logic reads ----------------------------------
    pub GRASS: Block = "petramond:grass";
    pub DIRT: Block = "petramond:dirt";
    pub WATER: Block = "petramond:water";
    pub SHORT_GRASS: Block = "petramond:short_grass";
    pub FERN: Block = "petramond:fern";
    pub DEAD_BUSH: Block = "petramond:dead_bush";
    /// The engine's wild hemp stand.
    pub HEMP_WILD: Block = "petramond:hemp";
    pub OAK_SAPLING: Block = "petramond:oak_sapling";
    pub OAK_SAPLING_1: Block = "petramond:oak_sapling_1";
    pub OAK_SAPLING_2: Block = "petramond:oak_sapling_2";
    pub SPRUCE_SAPLING: Block = "petramond:spruce_sapling";
    pub SPRUCE_SAPLING_1: Block = "petramond:spruce_sapling_1";
    pub SPRUCE_SAPLING_2: Block = "petramond:spruce_sapling_2";
    pub BIRCH_SAPLING: Block = "petramond:birch_sapling";
    pub BIRCH_SAPLING_1: Block = "petramond:birch_sapling_1";
    pub BIRCH_SAPLING_2: Block = "petramond:birch_sapling_2";
    pub JUNGLE_SAPLING: Block = "petramond:jungle_sapling";
    pub JUNGLE_SAPLING_1: Block = "petramond:jungle_sapling_1";
    pub JUNGLE_SAPLING_2: Block = "petramond:jungle_sapling_2";
    pub ACACIA_SAPLING: Block = "petramond:acacia_sapling";
    pub ACACIA_SAPLING_1: Block = "petramond:acacia_sapling_1";
    pub ACACIA_SAPLING_2: Block = "petramond:acacia_sapling_2";

    // --- items -----------------------------------------------------------
    pub IRON_HOE: Item = "farming:iron_hoe";
    pub FERTILIZER: Item = "farming:fertilizer";
    pub WHEAT: Item = "farming:wheat";
    pub WHEAT_SEEDS: Item = "farming:wheat_seeds";
    pub CARROT: Item = "farming:carrot";
    pub POTATO: Item = "farming:potato";
    pub HEMP_SEEDS: Item = "farming:hemp_seeds";
    /// The engine's hemp: cultivated hemp's produce.
    pub HEMP: Item = "petramond:hemp";

    // --- mobs, sounds, bursts, effect, recipe ----------------------------
    pub RABBIT: Mob = "farming:rabbit";
    pub TILL_SOUND: Sound = "farming:till";
    pub HARVEST_SOUND: Sound = "farming:harvest";
    pub TILL_BURST: Emitter = "farming:till_burst";
    pub COMPOST_FILL: Emitter = "farming:compost_fill";
    pub FERTILIZE_BURST: Emitter = "farming:fertilize_burst";
    pub WHEAT_HARVEST: Emitter = "farming:wheat_harvest";
    pub CARROT_HARVEST: Emitter = "farming:carrot_harvest";
    pub POTATO_HARVEST: Emitter = "farming:potato_harvest";
    pub WELL_FED: Effect = "farming:well_fed";
    pub FARMERS_WORKBENCH: Recipe = "farming:farmers_workbench";

    // --- tags, data, hooks, brain nodes ----------------------------------
    /// Any pack opts its scraps into the compost barrel with this item tag.
    pub COMPOSTABLE_TAG: Tag = "farming:compostable";
    pub SAPLING_TAG: Tag = "petramond:sapling";
    pub ROOTS_IN_SOIL_TAG: Tag = "petramond:roots_in_soil";
    /// The species-row data a breedable animal carries.
    pub HUSBANDRY_DATA: Data = "farming:husbandry";
    pub CROP_DATA: Data = "farming:crop";
    pub WILD_PATCH_DATA: Data = "farming:wild_patch";
    pub CROP_HOOK: Behavior = "farming:crop";
    pub FARMLAND_HOOK: Behavior = "farming:farmland";
    pub SPREAD_HOOK: Behavior = "farming:grass_fertilized";
    pub FOLLOW_WHEAT_NODE: AiNode = "farming:follow_wheat";
    pub HUSBANDRY_GOAL_NODE: AiNode = "farming:husbandry_goal";
}

/// Each engine sapling species' stage rows, seedling to final.
pub const SAPLINGS: [[&str; 3]; 5] = [
    [OAK_SAPLING, OAK_SAPLING_1, OAK_SAPLING_2],
    [SPRUCE_SAPLING, SPRUCE_SAPLING_1, SPRUCE_SAPLING_2],
    [BIRCH_SAPLING, BIRCH_SAPLING_1, BIRCH_SAPLING_2],
    [JUNGLE_SAPLING, JUNGLE_SAPLING_1, JUNGLE_SAPLING_2],
    [ACACIA_SAPLING, ACACIA_SAPLING_1, ACACIA_SAPLING_2],
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS, mod_sdk::WATER_BUCKET_KEYS]);
    }
}
