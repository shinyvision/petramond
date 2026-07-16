//! The pack's registry names resolved to session ids, once, at init.
//!
//! Numeric ids are session-scoped (never persisted); every other module works
//! against this struct instead of re-resolving names or — worse — hardcoding
//! numbers. Resolution is registry-only (`resolve_block`/`resolve_item`), so
//! it also runs on detached worldgen instances.

use mod_sdk::*;

/// Which cultivated crop a block belongs to.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Crop {
    Wheat,
    Carrots,
}

impl Crop {
    /// The item that replants this crop — what a broken support returns.
    pub fn planting_stock(self) -> &'static str {
        match self {
            Crop::Wheat => "farming:wheat_seeds",
            Crop::Carrots => "farming:carrot",
        }
    }
}

pub struct Content {
    // Pack blocks.
    pub farmland_dry: BlockId,
    pub farmland_wet: BlockId,
    pub wild_wheat: BlockId,
    pub wild_carrots: BlockId,
    /// Cultivated wheat stages 0..=3 (seedling..mature).
    pub wheat: [BlockId; 4],
    /// Cultivated carrot stages 0..=3.
    pub carrots: [BlockId; 4],
    // Engine blocks the logic reads.
    pub grass: BlockId,
    pub dirt: BlockId,
    pub water: BlockId,
    /// Ground vegetation tilling/worldgen may replace (short grass, fern,
    /// dead bush) — the walk-through cover plants, never crops or structures.
    pub clearable: [BlockId; 3],
    // Pack items.
    pub iron_hoe: ItemId,
}

impl Content {
    pub fn resolve() -> Option<Content> {
        let block = resolve_block_logged;
        let item = resolve_item_logged;
        Some(Content {
            farmland_dry: block("farming:farmland_dry")?,
            farmland_wet: block("farming:farmland_wet")?,
            wild_wheat: block("farming:wild_wheat")?,
            wild_carrots: block("farming:wild_carrots")?,
            wheat: [
                block("farming:wheat_0")?,
                block("farming:wheat_1")?,
                block("farming:wheat_2")?,
                block("farming:wheat_3")?,
            ],
            carrots: [
                block("farming:carrots_0")?,
                block("farming:carrots_1")?,
                block("farming:carrots_2")?,
                block("farming:carrots_3")?,
            ],
            grass: block("petramond:grass")?,
            dirt: block("petramond:dirt")?,
            water: block("petramond:water")?,
            clearable: [
                block("petramond:short_grass")?,
                block("petramond:fern")?,
                block("petramond:dead_bush")?,
            ],
            iron_hoe: item("farming:iron_hoe")?,
        })
    }

    /// Both farmland variants support planting and crops; wet/dry is a
    /// visual distinction only (growth checks REAL hydration).
    pub fn is_farmland(&self, b: BlockId) -> bool {
        b == self.farmland_dry || b == self.farmland_wet
    }

    /// The cultivated crop + stage a block id encodes, if it is one.
    pub fn crop_stage(&self, b: BlockId) -> Option<(Crop, u8)> {
        if let Some(i) = self.wheat.iter().position(|&s| s == b) {
            return Some((Crop::Wheat, i as u8));
        }
        if let Some(i) = self.carrots.iter().position(|&s| s == b) {
            return Some((Crop::Carrots, i as u8));
        }
        None
    }

    /// The block id for a crop's stage.
    pub fn stage_block(&self, crop: Crop, stage: u8) -> BlockId {
        match crop {
            Crop::Wheat => self.wheat[stage as usize],
            Crop::Carrots => self.carrots[stage as usize],
        }
    }

    pub fn is_clearable_cover(&self, b: BlockId) -> bool {
        self.clearable.contains(&b)
    }
}
