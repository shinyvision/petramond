//! The pack's registry names resolved to session ids, once, at init.
//!
//! Numeric ids are session-scoped (never persisted); every other module works
//! against this struct instead of re-resolving names or — worse — hardcoding
//! numbers. Resolution is registry-only (`resolve_block`/`resolve_item`), so
//! it also runs on detached worldgen instances.

use mod_sdk::*;

/// The static per-crop row everything else derives from. Adding a crop is
/// ONE row here (+ the pack JSON): stages, items, RNG keys, and the harvest
/// emitter all resolve from it in [`Content::resolve`] — never a new match
/// arm anywhere.
struct CropSpec {
    /// Singular stem: derives the RNG stream keys (`harvest_<name>`,
    /// `fertile_<name>`) and the harvest burst (`farming:<name>_harvest`).
    name: &'static str,
    /// Block-row stem: stages are `farming:<block_stem>_0..3`.
    block_stem: &'static str,
    /// The item that replants this crop — what a broken support returns.
    planting_stock: &'static str,
    /// Primary produce item + its per-harvest yield range (balance data).
    produce: &'static str,
    yield_range: (u64, u64),
    /// An optional secondary drop per harvest: (RNG stream key, item, lo,
    /// hi). The key is a literal so existing streams stay byte-identical.
    extra_drop: Option<(&'static str, &'static str, u64, u64)>,
}

const CROPS: &[CropSpec] = &[
    CropSpec {
        name: "wheat",
        block_stem: "wheat",
        planting_stock: "farming:wheat_seeds",
        produce: "farming:wheat",
        yield_range: (1, 2),
        extra_drop: Some(("harvest_wheat_seeds", "farming:wheat_seeds", 0, 2)),
    },
    CropSpec {
        name: "carrot",
        block_stem: "carrots",
        planting_stock: "farming:carrot",
        produce: "farming:carrot",
        yield_range: (2, 3),
        extra_drop: None,
    },
    CropSpec {
        name: "potato",
        block_stem: "potatoes",
        planting_stock: "farming:potato",
        produce: "farming:potato",
        yield_range: (2, 3),
        extra_drop: None,
    },
];

/// One cultivated crop, resolved: the stage blocks plus everything a harvest
/// or a support pop needs, derived from its [`CropSpec`] row.
pub struct CropDef {
    /// Growth stages 0..=3 (seedling..mature). Stage identity IS the block.
    pub stages: [BlockId; 4],
    pub planting_stock: &'static str,
    pub produce: &'static str,
    pub yield_range: (u64, u64),
    pub extra_drop: Option<(&'static str, &'static str, u64, u64)>,
    /// RNG stream keys (derived once from the spec name — streams are
    /// stateful per key, so these must never vary per call site).
    pub harvest_key: String,
    pub fertile_key: String,
    pub harvest_emitter: String,
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
    // Pack items.
    pub iron_hoe: ItemId,
    pub fertilizer: ItemId,
    /// The wheat item — the sheep lure (see [`crate::follow`]).
    pub wheat_item: ItemId,
    /// Everything carrying the `farming:compostable` item tag — any pack may
    /// opt its own scraps into the barrel by listing the tag on a row.
    pub compostable: Vec<ItemId>,
}

impl Content {
    pub fn resolve() -> Option<Content> {
        let block = resolve_block_logged;
        let item = resolve_item_logged;
        let short_grass = block("petramond:short_grass")?;
        let fern = block("petramond:fern")?;
        let dead_bush = block("petramond:dead_bush")?;
        // Saplings (every growth-stage row carries the engine `sapling` tag)
        // are excluded from vegetation spread — they get the growth boost.
        let saplings = blocks_by_tag("petramond:sapling");
        let spreadable: Vec<BlockId> = blocks_by_tag("petramond:roots_in_soil")
            .into_iter()
            .filter(|b| !saplings.contains(b))
            .collect();
        let mut sapling_finals = Vec::new();
        for species in ["oak", "spruce", "birch", "jungle", "acacia"] {
            let last = block(&format!("petramond:{species}_sapling_2"))?;
            sapling_finals.push((block(&format!("petramond:{species}_sapling"))?, last));
            sapling_finals.push((block(&format!("petramond:{species}_sapling_1"))?, last));
            sapling_finals.push((last, last));
        }
        let mut crops = Vec::with_capacity(CROPS.len());
        for spec in CROPS {
            let mut stages = [BlockId::AIR; 4];
            for (i, stage) in stages.iter_mut().enumerate() {
                *stage = block(&format!("farming:{}_{i}", spec.block_stem))?;
            }
            crops.push(CropDef {
                stages,
                planting_stock: spec.planting_stock,
                produce: spec.produce,
                yield_range: spec.yield_range,
                extra_drop: spec.extra_drop,
                harvest_key: format!("harvest_{}", spec.name),
                fertile_key: format!("fertile_{}", spec.name),
                harvest_emitter: format!("farming:{}_harvest", spec.name),
            });
        }
        Some(Content {
            farmland_dry: block("farming:farmland_dry")?,
            farmland_wet: block("farming:farmland_wet")?,
            farmland_fertile_dry: block("farming:farmland_fertile_dry")?,
            farmland_fertile_wet: block("farming:farmland_fertile_wet")?,
            wild_wheat: block("farming:wild_wheat")?,
            wild_carrots: block("farming:wild_carrots")?,
            wild_potatoes: block("farming:wild_potatoes")?,
            crops,
            compost: [
                block("farming:compost_0")?,
                block("farming:compost_1")?,
                block("farming:compost_2")?,
                block("farming:compost_3")?,
            ],
            grass_fertilized: block("farming:grass_fertilized")?,
            grass: block("petramond:grass")?,
            dirt: block("petramond:dirt")?,
            water: block("petramond:water")?,
            sapling_finals,
            spreadable,
            clearable: [short_grass, fern, dead_bush],
            seed_cover: [short_grass, fern],
            iron_hoe: item("farming:iron_hoe")?,
            fertilizer: item("farming:fertilizer")?,
            wheat_item: item("farming:wheat")?,
            compostable: items_by_tag("farming:compostable"),
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
