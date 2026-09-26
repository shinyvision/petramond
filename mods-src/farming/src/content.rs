//! The pack's registry names resolved to session ids, once, at init.
//!
//! Numeric ids are session-scoped (never persisted); every other module works
//! against this struct instead of re-resolving names or — worse — hardcoding
//! numbers. Resolution is registry-only (`resolve_block`/`resolve_item`), so
//! it also runs on detached worldgen instances.

use mod_sdk::*;

use crate::keys;

/// The per-crop data on each mature block's `farming:crop` row.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CropSpec {
    /// Singular stem: derives the RNG stream keys (`harvest_<name>`,
    /// `fertile_<name>`).
    name: String,
    /// The stage rows, seedling (0) to mature (3).
    stages: [String; 4],
    /// The burst a harvest plays, if the pack ships one for this crop.
    harvest_emitter: Option<String>,
    /// The item that replants this crop — what a broken support returns.
    planting_stock: String,
    /// Primary produce item + its per-harvest yield range (balance data).
    produce: String,
    yield_range: (u64, u64),
    /// An optional secondary drop per harvest.
    extra_drop: Option<ExtraDrop>,
    /// The species a PLANTED stand of this crop draws out of the wild
    /// (`mobs.json` row key), if any — see [`crate::attract`]. Only cultivated
    /// rows attract: the wild stands are already where the animal lives.
    attracts: Option<String>,
}

/// A crop's secondary harvest drop — the seeds a tended plant throws off
/// beside its produce.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtraDrop {
    /// RNG stream key for the COUNT. A frozen literal: streams are stateful
    /// per key, so an existing one must never be renamed.
    count_key: String,
    item: String,
    /// Percent of harvests that yield it at all. `100` means every harvest,
    /// and draws no chance roll — so a crop that always rolls keeps its
    /// historic stream exactly as it was before chances existed.
    chance_percent: u64,
    /// How many, inclusive, when it does yield.
    count: (u64, u64),
}

mod husbandry;
pub use husbandry::{Eaten, HusbandryDef};

const CROP_KEY: &str = keys::CROP_DATA;

/// A resolved [`ExtraDrop`]: the spec's frozen count stream plus the derived
/// chance stream, so the two rolls never share state.
pub struct ExtraDropDef {
    pub item: String,
    pub chance_percent: u64,
    pub count: (u64, u64),
    pub count_key: String,
    pub chance_key: String,
}

/// One cultivated crop, resolved: the stage blocks plus everything a harvest
/// or a support pop needs, derived from its [`CropSpec`] row.
pub struct CropDef {
    /// Growth stages 0..=3 (seedling..mature). Stage identity IS the block.
    pub stages: [BlockId; 4],
    pub planting_stock: String,
    pub produce: String,
    pub yield_range: (u64, u64),
    pub extra_drop: Option<ExtraDropDef>,
    /// The species a planted stand draws in, key and resolved id (see
    /// [`crate::attract`]).
    pub attracts: Option<(String, MobId)>,
    /// RNG stream keys (derived once from the spec name — streams are
    /// stateful per key, so these must never vary per call site).
    pub harvest_key: String,
    pub fertile_key: String,
    pub harvest_emitter: Option<String>,
    /// The attraction roll's own RNG stream key — never shared with the
    /// harvest streams, which are stateful per key.
    pub attract_key: String,
}

pub struct Content {
    // Pack blocks.
    pub farmland_dry: BlockId,
    pub farmland_wet: BlockId,
    /// Fertilized soil, same wet/dry visual pair: crops on it grow faster
    /// and yield a harvest bonus (see [`crate::crops`]).
    pub farmland_fertile_dry: BlockId,
    pub farmland_fertile_wet: BlockId,
    pub wild_wheat: BlockId,
    pub wild_carrots: BlockId,
    pub wild_potatoes: BlockId,
    /// Ordered wild patch rules read from the wild block rows.
    pub(crate) wild_patches: Vec<crate::worldgen::WildCropSpec>,
    /// Cultivated crops read from mature block rows carrying `farming:crop`.
    pub crops: Vec<CropDef>,
    /// Compost barrel fill stages 0..=3 (empty..full).
    pub compost: [BlockId; 4],
    /// Empty water trough (water + wheat cubes hidden).
    pub trough: BlockId,
    /// Filled water trough (water cube visible).
    pub trough_filled: BlockId,
    /// Wheat-filled trough (wheat bed + stalks visible).
    pub trough_wheat: BlockId,
    /// Fertilized grass: spreads its rooted vegetation to nearby grass for a
    /// while, then relaxes back to plain grass (see [`crate::spread`]).
    pub grass_fertilized: BlockId,
    // Engine blocks the logic reads.
    pub grass: BlockId,
    pub dirt: BlockId,
    pub water: BlockId,
    /// Growth-boost table: each engine sapling stage row → its species' FINAL
    /// stage row (the engine's stage-row chain, resolved by name like the
    /// other engine blocks above). A final row maps to itself — that identity
    /// is how "already boosted" is detected without wasting a unit.
    pub sapling_finals: Vec<(BlockId, BlockId)>,
    /// Vegetation fertilized grass propagates: everything that roots in soil
    /// (flowers, short grass, ferns, mushrooms — pack rows included via the
    /// tag) EXCEPT saplings, which get the growth boost instead.
    pub spreadable: Vec<BlockId>,
    /// Ground vegetation tilling/worldgen may replace (short grass, fern,
    /// dead bush) — the walk-through cover plants, never crops or structures.
    pub clearable: [BlockId; 3],
    /// LIVING ground cover (short grass, fern): breaking it can forage a
    /// stray wheat seed — a dead bush holds none (see [`crate::forage`]).
    pub seed_cover: [BlockId; 2],
    /// The ENGINE's wild hemp stand. The pack adds a seed roll to it because a
    /// core row is one this pack cannot give drops to; every CULTIVATED stage
    /// declares its own drops in `blocks.json` (see [`crate::hemp`]).
    pub hemp_wild: BlockId,
    // Pack items.
    pub iron_hoe: ItemId,
    pub fertilizer: ItemId,
    /// The wheat item — the sheep lure (see [`crate::follow`]).
    pub wheat_item: ItemId,
    /// Everything carrying the `farming:compostable` item tag — any pack may
    /// opt its own scraps into the barrel by listing the tag on a row.
    pub compostable: Vec<ItemId>,
    /// The engine's bucket pair — the trough's fill/drain items.
    pub buckets: WaterBuckets,
    /// Species carrying the husbandry consumer-data entry.
    pub husbandry: Vec<HusbandryDef>,
    /// The pack's rabbit — the hop gait's species (see [`crate::hop`]).
    pub rabbit: MobId,
}

impl Content {
    pub fn resolve() -> Option<Content> {
        let block = resolve_block_logged;
        let item = resolve_item_logged;
        let short_grass = block(keys::SHORT_GRASS)?;
        let fern = block(keys::FERN)?;
        let dead_bush = block(keys::DEAD_BUSH)?;
        // Saplings (every growth-stage row carries the engine `sapling` tag)
        // are excluded from vegetation spread — they get the growth boost.
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
        // The hemp a break can shake seeds out of. The wild stand is an ENGINE
        // row; the mature stage comes off the crop def, so a rename there
        // cannot leave this list behind. Seedlings are deliberately absent.
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

    /// Every farmland variant supports planting and crops; wet/dry is a
    /// visual distinction only (growth checks REAL hydration).
    pub fn is_farmland(&self, b: BlockId) -> bool {
        b == self.farmland_dry
            || b == self.farmland_wet
            || b == self.farmland_fertile_dry
            || b == self.farmland_fertile_wet
    }

    /// Fertilized soil (either skin): crops grow faster and yield a bonus.
    pub fn is_fertile(&self, b: BlockId) -> bool {
        b == self.farmland_fertile_dry || b == self.farmland_fertile_wet
    }

    /// The wet/dry skin pair for a farmland block, fertility preserved.
    pub fn farmland_skins(&self, b: BlockId) -> Option<(BlockId, BlockId)> {
        if self.is_fertile(b) {
            Some((self.farmland_fertile_dry, self.farmland_fertile_wet))
        } else if self.is_farmland(b) {
            Some((self.farmland_dry, self.farmland_wet))
        } else {
            None
        }
    }

    /// The cultivated crop + stage a block id encodes, if it is one.
    pub fn crop_stage(&self, b: BlockId) -> Option<(&CropDef, u8)> {
        self.crops.iter().find_map(|def| {
            def.stages
                .iter()
                .position(|&s| s == b)
                .map(|i| (def, i as u8))
        })
    }

    /// The stage BELOW `b` for a cultivated crop — what a grazing bite knocks
    /// it back to. `None` for a seedling (nothing below stage 0) and for
    /// anything that is not a crop stage at all.
    pub fn crop_regressed(&self, b: BlockId) -> Option<BlockId> {
        let (def, stage) = self.crop_stage(b)?;
        stage.checked_sub(1).map(|below| def.stages[below as usize])
    }

    /// The compost barrel's fill stage (0 = empty, 3 = full), if `b` is one.
    pub fn compost_stage(&self, b: BlockId) -> Option<u8> {
        self.compost.iter().position(|&s| s == b).map(|i| i as u8)
    }

    pub fn is_clearable_cover(&self, b: BlockId) -> bool {
        self.clearable.contains(&b)
    }

    /// The FINAL growth stage of a known sapling stage row (`None` = not a
    /// sapling this pack knows how to boost). `Some(b)` for a final row itself
    /// — the caller compares to detect "already boosted".
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
        assert_eq!(rows.len(), 4);
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
