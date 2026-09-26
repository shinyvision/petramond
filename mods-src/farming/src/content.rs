//! The pack's registry names resolved to session ids, once, at init.
//!
//! Numeric ids are session-scoped (never persisted); every other module works
//! against this struct instead of re-resolving names or — worse — hardcoding
//! numbers. Resolution is registry-only (`resolve_block`/`resolve_item`), so
//! it also runs on detached worldgen instances.

use mod_sdk::*;

use crate::keys;

/// The static per-crop row everything else derives from. Adding a crop is
/// ONE row here (+ its ids in [`crate::keys`] and the pack JSON): stages,
/// items, RNG keys, and the harvest emitter all resolve from it in
/// [`Content::resolve`] — never a new match arm anywhere.
struct CropSpec {
    /// Singular stem: derives the RNG stream keys (`harvest_<name>`,
    /// `fertile_<name>`).
    name: &'static str,
    /// The stage rows, seedling (0) to mature (3).
    stages: [&'static str; 4],
    /// The burst a harvest plays, if the pack ships one for this crop.
    harvest_emitter: Option<&'static str>,
    /// The item that replants this crop — what a broken support returns.
    planting_stock: &'static str,
    /// Primary produce item + its per-harvest yield range (balance data).
    produce: &'static str,
    yield_range: (u64, u64),
    /// An optional secondary drop per harvest.
    extra_drop: Option<ExtraDrop>,
    /// The species a PLANTED stand of this crop draws out of the wild
    /// (`mobs.json` row key), if any — see [`crate::attract`]. Only cultivated
    /// rows attract: the wild stands are already where the animal lives.
    attracts: Option<&'static str>,
}

/// A crop's secondary harvest drop — the seeds a tended plant throws off
/// beside its produce.
struct ExtraDrop {
    /// RNG stream key for the COUNT. A frozen literal: streams are stateful
    /// per key, so an existing one must never be renamed.
    count_key: &'static str,
    item: &'static str,
    /// Percent of harvests that yield it at all. `100` means every harvest,
    /// and draws no chance roll — so a crop that always rolls keeps its
    /// historic stream exactly as it was before chances existed.
    chance_percent: u64,
    /// How many, inclusive, when it does yield.
    count: (u64, u64),
}

mod husbandry;
pub use husbandry::{Eaten, HusbandryDef};

const CROPS: &[CropSpec] = &[
    CropSpec {
        name: "wheat",
        stages: [keys::WHEAT_0, keys::WHEAT_1, keys::WHEAT_2, keys::WHEAT_3],
        harvest_emitter: Some(keys::WHEAT_HARVEST),
        planting_stock: keys::WHEAT_SEEDS,
        produce: keys::WHEAT,
        yield_range: (1, 2),
        attracts: None,
        extra_drop: Some(ExtraDrop {
            count_key: "harvest_wheat_seeds",
            item: keys::WHEAT_SEEDS,
            chance_percent: 100,
            count: (0, 2),
        }),
    },
    CropSpec {
        name: "carrot",
        stages: [
            keys::CARROTS_0,
            keys::CARROTS_1,
            keys::CARROTS_2,
            keys::CARROTS_3,
        ],
        harvest_emitter: Some(keys::CARROT_HARVEST),
        planting_stock: keys::CARROT,
        produce: keys::CARROT,
        yield_range: (2, 3),
        extra_drop: None,
        // A planted carrot patch is what brings rabbits in from the wild.
        attracts: Some(keys::RABBIT),
    },
    CropSpec {
        name: "potato",
        stages: [
            keys::POTATOES_0,
            keys::POTATOES_1,
            keys::POTATOES_2,
            keys::POTATOES_3,
        ],
        harvest_emitter: Some(keys::POTATO_HARVEST),
        planting_stock: keys::POTATO,
        produce: keys::POTATO,
        yield_range: (2, 3),
        extra_drop: None,
        attracts: None,
    },
    // Hemp is the one crop whose stock and produce are ENGINE items: the wild
    // stands and the rope they lash the first stone tools with are core
    // progression, so cultivating it is this pack making a core material
    // renewable, not owning it. The pack ships no hemp harvest burst.
    CropSpec {
        name: "hemp",
        stages: [keys::HEMP_0, keys::HEMP_1, keys::HEMP_2, keys::HEMP_3],
        harvest_emitter: None,
        planting_stock: keys::HEMP_SEEDS,
        produce: keys::HEMP,
        yield_range: (1, 1),
        attracts: None,
        extra_drop: Some(ExtraDrop {
            count_key: "harvest_hemp_seeds",
            item: keys::HEMP_SEEDS,
            chance_percent: 60,
            count: (1, 2),
        }),
    },
];

/// A resolved [`ExtraDrop`]: the spec's frozen count stream plus the derived
/// chance stream, so the two rolls never share state.
pub struct ExtraDropDef {
    pub item: &'static str,
    pub chance_percent: u64,
    pub count: (u64, u64),
    pub count_key: &'static str,
    pub chance_key: String,
}

/// One cultivated crop, resolved: the stage blocks plus everything a harvest
/// or a support pop needs, derived from its [`CropSpec`] row.
pub struct CropDef {
    /// Growth stages 0..=3 (seedling..mature). Stage identity IS the block.
    pub stages: [BlockId; 4],
    pub planting_stock: &'static str,
    pub produce: &'static str,
    pub yield_range: (u64, u64),
    pub extra_drop: Option<ExtraDropDef>,
    /// The species a planted stand draws in, key and resolved id (see
    /// [`crate::attract`]).
    pub attracts: Option<(&'static str, MobId)>,
    /// RNG stream keys (derived once from the spec name — streams are
    /// stateful per key, so these must never vary per call site).
    pub harvest_key: String,
    pub fertile_key: String,
    pub harvest_emitter: Option<&'static str>,
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
    /// The cultivated crops, one [`CropDef`] per [`CROPS`] row.
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
    /// Empty wooden bucket.
    pub wooden_bucket: ItemId,
    /// Water-filled wooden bucket.
    pub water_bucket: ItemId,
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
        let mut crops = Vec::with_capacity(CROPS.len());
        for spec in CROPS {
            let mut stages = [BlockId::AIR; 4];
            for (stage, name) in stages.iter_mut().zip(spec.stages) {
                *stage = block(name)?;
            }
            crops.push(CropDef {
                stages,
                planting_stock: spec.planting_stock,
                produce: spec.produce,
                yield_range: spec.yield_range,
                extra_drop: spec.extra_drop.as_ref().map(|e| ExtraDropDef {
                    item: e.item,
                    chance_percent: e.chance_percent,
                    count: e.count,
                    count_key: e.count_key,
                    chance_key: format!("extra_chance_{}", spec.name),
                }),
                attracts: match spec.attracts {
                    None => None,
                    Some(key) => match resolve_mob(key) {
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
            wooden_bucket: item(keys::WOODEN_BUCKET)?,
            water_bucket: item(keys::WATER_BUCKET)?,
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
