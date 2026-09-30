use mod_sdk::*;

use crate::keys;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CropSpec {
    name: String,
    stages: [String; 4],
    harvest_emitter: Option<String>,
    planting_stock: String,
    produce: String,
    yield_range: (u64, u64),
    extra_drop: Option<ExtraDrop>,
    attracts: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtraDrop {
    count_key: String,
    item: String,
    chance_percent: u64,
    count: (u64, u64),
}

mod husbandry;
pub use husbandry::{Eaten, HusbandryDef};

const CROP_KEY: &str = keys::CROP_DATA;

pub struct ExtraDropDef {
    pub item: String,
    pub chance_percent: u64,
    pub count: (u64, u64),
    pub count_key: String,
    pub chance_key: String,
}

pub struct CropDef {
    pub stages: [BlockId; 4],
    pub planting_stock: String,
    pub produce: String,
    pub yield_range: (u64, u64),
    pub extra_drop: Option<ExtraDropDef>,
    pub attracts: Option<(String, MobId)>,
    pub harvest_key: String,
    pub fertile_key: String,
    pub harvest_emitter: Option<String>,
    pub attract_key: String,
}

pub struct Content {
    pub farmland_dry: BlockId,
    pub farmland_wet: BlockId,
    pub farmland_fertile_dry: BlockId,
    pub farmland_fertile_wet: BlockId,
    #[allow(dead_code)]
    pub wild_wheat: BlockId,
    #[allow(dead_code)]
    pub wild_carrots: BlockId,
    #[allow(dead_code)]
    pub wild_potatoes: BlockId,
    pub(crate) wild_patches: Vec<crate::worldgen::WildCropSpec>,
    pub crops: Vec<CropDef>,
    pub compost: [BlockId; 4],
    pub trough: BlockId,
    pub trough_filled: BlockId,
    pub trough_wheat: BlockId,
    pub grass_fertilized: BlockId,
    pub grass: BlockId,
    pub dirt: BlockId,
    pub water: BlockId,
    pub sapling_finals: Vec<(BlockId, BlockId)>,
    pub spreadable: Vec<BlockId>,
    pub clearable: [BlockId; 3],
    pub seed_cover: [BlockId; 2],
    pub hemp_wild: BlockId,
    pub iron_hoe: ItemId,
    pub fertilizer: ItemId,
    pub wheat_item: ItemId,
    pub compostable: Vec<ItemId>,
    pub buckets: WaterBuckets,
    pub husbandry: Vec<HusbandryDef>,
    pub rabbit: MobId,
}

impl Content {
    pub fn resolve() -> Option<Content> {
        let block = resolve_block_logged;
        let item = resolve_item_logged;
        let short_grass = block(keys::SHORT_GRASS)?;
        let fern = block(keys::FERN)?;
        let dead_bush = block(keys::DEAD_BUSH)?;
        let saplings = blocks_by_tag(keys::SAPLING_TAG);
        let spreadable: Vec<BlockId> = blocks_by_tag(keys::ROOTS_IN_SOIL_TAG)
            .into_iter()
            .filter(|b| !saplings.contains(b))
            .collect();
        let mut sapling_finals = Vec::new();
        for [seedling, middle, last] in keys::SAPLINGS {
            let last = block(last)?;
            sapling_finals.push((block(seedling)?, last));
            sapling_finals.push((block(middle)?, last));
            sapling_finals.push((last, last));
        }
        let husbandry = husbandry::resolve();
        let mut crops = Vec::new();
        for (mature, spec) in blocks_with_data_as::<CropSpec>(CROP_KEY) {
            let mut stages = [BlockId::AIR; 4];
            for (stage, name) in stages.iter_mut().zip(&spec.stages) {
                *stage = block(name)?;
            }
            if stages[3] != mature
                || spec.yield_range.0 > spec.yield_range.1
                || spec.yield_range.1 >= u64::from(u8::MAX)
                || spec.extra_drop.as_ref().is_some_and(|extra| {
                    extra.count.0 > extra.count.1
                        || extra.count.1 > u64::from(u8::MAX)
                        || extra.chance_percent > 100
                })
            {
                log(&format!("farming: invalid {CROP_KEY} row for {mature:?}"));
                return None;
            }
            crops.push(CropDef {
                stages,
                planting_stock: spec.planting_stock,
                produce: spec.produce,
                yield_range: spec.yield_range,
                extra_drop: spec.extra_drop.map(|e| ExtraDropDef {
                    item: e.item,
                    chance_percent: e.chance_percent,
                    count: e.count,
                    count_key: e.count_key,
                    chance_key: format!("extra_chance_{}", spec.name),
                }),
                attracts: match spec.attracts {
                    None => None,
                    Some(key) => match resolve_mob(&key) {
                        Some(kind) => Some((key, kind)),
                        None => {
                            log(&format!("farming: unknown attracted species '{key}'"));
                            return None;
                        }
                    },
                },
                harvest_key: format!("harvest_{}", spec.name),
                fertile_key: format!("fertile_{}", spec.name),
                harvest_emitter: spec.harvest_emitter,
                attract_key: format!("attract_{}", spec.name),
            });
        }
        let mut hemp_mature = vec![block(keys::HEMP_WILD)?];
        if let Some(def) = crops.iter().find(|c| c.planting_stock == keys::HEMP_SEEDS) {
            hemp_mature.push(def.stages[3]);
        }
        Some(Content {
            farmland_dry: block(keys::FARMLAND_DRY)?,
            farmland_wet: block(keys::FARMLAND_WET)?,
            farmland_fertile_dry: block(keys::FARMLAND_FERTILE_DRY)?,
            farmland_fertile_wet: block(keys::FARMLAND_FERTILE_WET)?,
            wild_wheat: block(keys::WILD_WHEAT)?,
            wild_carrots: block(keys::WILD_CARROTS)?,
            wild_potatoes: block(keys::WILD_POTATOES)?,
            wild_patches: crate::worldgen::resolve_specs(),
            crops,
            compost: [
                block(keys::COMPOST_0)?,
                block(keys::COMPOST_1)?,
                block(keys::COMPOST_2)?,
                block(keys::COMPOST_3)?,
            ],
            trough: block(keys::TROUGH)?,
            trough_filled: block(keys::TROUGH_FILLED)?,
            trough_wheat: block(keys::TROUGH_WHEAT)?,
            grass_fertilized: block(keys::GRASS_FERTILIZED)?,
            grass: block(keys::GRASS)?,
            dirt: block(keys::DIRT)?,
            water: block(keys::WATER)?,
            sapling_finals,
            spreadable,
            clearable: [short_grass, fern, dead_bush],
            seed_cover: [short_grass, fern],
            hemp_wild: block(keys::HEMP_WILD)?,
            iron_hoe: item(keys::IRON_HOE)?,
            fertilizer: item(keys::FERTILIZER)?,
            wheat_item: item(keys::WHEAT)?,
            compostable: items_by_tag(keys::COMPOSTABLE_TAG),
            buckets: WaterBuckets::resolve()?,
            husbandry,
            rabbit: resolve_mob_logged(keys::RABBIT)?,
        })
    }

    pub fn is_farmland(&self, b: BlockId) -> bool {
        b == self.farmland_dry
            || b == self.farmland_wet
            || b == self.farmland_fertile_dry
            || b == self.farmland_fertile_wet
    }

    pub fn is_fertile(&self, b: BlockId) -> bool {
        b == self.farmland_fertile_dry || b == self.farmland_fertile_wet
    }

    pub fn farmland_skins(&self, b: BlockId) -> Option<(BlockId, BlockId)> {
        if self.is_fertile(b) {
            Some((self.farmland_fertile_dry, self.farmland_fertile_wet))
        } else if self.is_farmland(b) {
            Some((self.farmland_dry, self.farmland_wet))
        } else {
            None
        }
    }

    pub fn crop_stage(&self, b: BlockId) -> Option<(&CropDef, u8)> {
        self.crops.iter().find_map(|def| {
            def.stages
                .iter()
                .position(|&s| s == b)
                .map(|i| (def, i as u8))
        })
    }

    pub fn crop_regressed(&self, b: BlockId) -> Option<BlockId> {
        let (def, stage) = self.crop_stage(b)?;
        stage.checked_sub(1).map(|below| def.stages[below as usize])
    }

    pub fn compost_stage(&self, b: BlockId) -> Option<u8> {
        self.compost.iter().position(|&s| s == b).map(|i| i as u8)
    }

    pub fn is_clearable_cover(&self, b: BlockId) -> bool {
        self.clearable.contains(&b)
    }

    pub fn sapling_final(&self, b: BlockId) -> Option<BlockId> {
        self.sapling_finals
            .iter()
            .find(|(from, _)| *from == b)
            .map(|&(_, last)| last)
    }
}

#[cfg(test)]
mod crop_rows_tests {
    use super::*;

    #[test]
    fn shipped_crop_rows_have_valid_stages_and_yields() {
        let rows = pack_rows_with_data(include_str!("../pack/blocks.json"), "blocks", CROP_KEY);
        for (mature, raw) in rows {
            let spec: CropSpec = parse_row_data(&raw).unwrap_or_else(|e| panic!("{mature}: {e}"));
            assert_eq!(spec.stages[3], mature);
            assert!(spec.yield_range.0 <= spec.yield_range.1);
            if let Some(extra) = spec.extra_drop {
                assert!(extra.count.0 <= extra.count.1, "{mature}");
                assert!(extra.chance_percent <= 100, "{mature}");
            }
        }
    }
}
