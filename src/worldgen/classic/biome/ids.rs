//! Biome id constants, climate-tag constants, classification helpers, and the
//! biome-selection arrays used by the cascade. These numeric ids are the output
//! values and the verification keys (they match the reference ruleset's ids).

// --- Climate tags (intermediate cell values before biome assignment) ---
pub const T_OCEANIC: i32 = 0;
pub const T_WARM: i32 = 1;
pub const T_LUSH: i32 = 2;
pub const T_COLD: i32 = 3;
pub const T_FREEZING: i32 = 4;

// --- Biome ids ---
pub const OCEAN: i32 = 0;
pub const PLAINS: i32 = 1;
pub const DESERT: i32 = 2;
pub const MOUNTAINS: i32 = 3;
pub const FOREST: i32 = 4;
pub const TAIGA: i32 = 5;
pub const SWAMP: i32 = 6;
pub const RIVER: i32 = 7;
pub const FROZEN_OCEAN: i32 = 10;
pub const FROZEN_RIVER: i32 = 11;
pub const SNOWY_TUNDRA: i32 = 12;
pub const SNOWY_MOUNTAINS: i32 = 13;
pub const MUSHROOM_FIELDS: i32 = 14;
pub const MUSHROOM_FIELD_SHORE: i32 = 15;
pub const BEACH: i32 = 16;
pub const DESERT_HILLS: i32 = 17;
pub const WOODED_HILLS: i32 = 18;
pub const TAIGA_HILLS: i32 = 19;
pub const MOUNTAIN_EDGE: i32 = 20;
pub const JUNGLE: i32 = 21;
pub const JUNGLE_HILLS: i32 = 22;
pub const JUNGLE_EDGE: i32 = 23;
pub const DEEP_OCEAN: i32 = 24;
pub const STONE_SHORE: i32 = 25;
pub const SNOWY_BEACH: i32 = 26;
pub const BIRCH_FOREST: i32 = 27;
pub const BIRCH_FOREST_HILLS: i32 = 28;
pub const DARK_FOREST: i32 = 29;
pub const SNOWY_TAIGA: i32 = 30;
pub const SNOWY_TAIGA_HILLS: i32 = 31;
pub const GIANT_TREE_TAIGA: i32 = 32;
pub const GIANT_TREE_TAIGA_HILLS: i32 = 33;
pub const WOODED_MOUNTAINS: i32 = 34;
pub const SAVANNA: i32 = 35;
pub const SAVANNA_PLATEAU: i32 = 36;
pub const BADLANDS: i32 = 37;
pub const WOODED_BADLANDS_PLATEAU: i32 = 38;
pub const BADLANDS_PLATEAU: i32 = 39;

/// Mutation marker bits (set by the special layer; cleared before biome lookup).
pub const MUTATION_MASK: i32 = 0xf00;

// --- Mutated ("M") biome variants (base id + 128), produced by the hills layer ---
pub const SUNFLOWER_PLAINS: i32 = PLAINS + 128;
pub const DESERT_LAKES: i32 = DESERT + 128;
pub const GRAVELLY_MOUNTAINS: i32 = MOUNTAINS + 128;
pub const FLOWER_FOREST: i32 = FOREST + 128;
pub const TAIGA_MOUNTAINS: i32 = TAIGA + 128;
pub const SWAMP_HILLS: i32 = SWAMP + 128;
pub const ICE_SPIKES: i32 = SNOWY_TUNDRA + 128;
pub const MODIFIED_JUNGLE: i32 = JUNGLE + 128;
pub const MODIFIED_JUNGLE_EDGE: i32 = JUNGLE_EDGE + 128;
pub const TALL_BIRCH_FOREST: i32 = BIRCH_FOREST + 128;
pub const TALL_BIRCH_HILLS: i32 = BIRCH_FOREST_HILLS + 128;
pub const DARK_FOREST_HILLS: i32 = DARK_FOREST + 128;
pub const SNOWY_TAIGA_MOUNTAINS: i32 = SNOWY_TAIGA + 128;
pub const GIANT_SPRUCE_TAIGA: i32 = GIANT_TREE_TAIGA + 128;
pub const GIANT_SPRUCE_TAIGA_HILLS: i32 = GIANT_TREE_TAIGA_HILLS + 128;
pub const MODIFIED_GRAVELLY_MOUNTAINS: i32 = WOODED_MOUNTAINS + 128;
pub const SHATTERED_SAVANNA: i32 = SAVANNA + 128;
pub const SHATTERED_SAVANNA_PLATEAU: i32 = SAVANNA_PLATEAU + 128;
pub const ERODED_BADLANDS: i32 = BADLANDS + 128;
pub const MODIFIED_WOODED_BADLANDS_PLATEAU: i32 = WOODED_BADLANDS_PLATEAU + 128;
pub const MODIFIED_BADLANDS_PLATEAU: i32 = BADLANDS_PLATEAU + 128;

/// The mutated ("M") variant of a biome, or [`NONE`] if it has none.
pub fn mutated(id: i32) -> i32 {
    match id {
        PLAINS => SUNFLOWER_PLAINS,
        DESERT => DESERT_LAKES,
        MOUNTAINS => GRAVELLY_MOUNTAINS,
        FOREST => FLOWER_FOREST,
        TAIGA => TAIGA_MOUNTAINS,
        SWAMP => SWAMP_HILLS,
        SNOWY_TUNDRA => ICE_SPIKES,
        JUNGLE => MODIFIED_JUNGLE,
        JUNGLE_EDGE => MODIFIED_JUNGLE_EDGE,
        BIRCH_FOREST => TALL_BIRCH_FOREST,
        BIRCH_FOREST_HILLS => TALL_BIRCH_HILLS,
        DARK_FOREST => DARK_FOREST_HILLS,
        SNOWY_TAIGA => SNOWY_TAIGA_MOUNTAINS,
        GIANT_TREE_TAIGA => GIANT_SPRUCE_TAIGA,
        GIANT_TREE_TAIGA_HILLS => GIANT_SPRUCE_TAIGA_HILLS,
        WOODED_MOUNTAINS => MODIFIED_GRAVELLY_MOUNTAINS,
        SAVANNA => SHATTERED_SAVANNA,
        SAVANNA_PLATEAU => SHATTERED_SAVANNA_PLATEAU,
        BADLANDS => ERODED_BADLANDS,
        WOODED_BADLANDS_PLATEAU => MODIFIED_WOODED_BADLANDS_PLATEAU,
        BADLANDS_PLATEAU => MODIFIED_BADLANDS_PLATEAU,
        _ => NONE,
    }
}

/// Badlands (mesa) family, including mutated variants.
pub fn is_mesa(id: i32) -> bool {
    matches!(
        id,
        BADLANDS
            | ERODED_BADLANDS
            | MODIFIED_WOODED_BADLANDS_PLATEAU
            | MODIFIED_BADLANDS_PLATEAU
            | WOODED_BADLANDS_PLATEAU
            | BADLANDS_PLATEAU
    )
}

// --- Biome-selection arrays (indexed by a bounded draw) ---
pub const WARM_BIOMES: [i32; 6] = [DESERT, DESERT, DESERT, SAVANNA, SAVANNA, PLAINS];
pub const LUSH_BIOMES: [i32; 6] = [FOREST, DARK_FOREST, MOUNTAINS, PLAINS, BIRCH_FOREST, SWAMP];
pub const COLD_BIOMES: [i32; 4] = [FOREST, MOUNTAINS, TAIGA, PLAINS];
pub const SNOW_BIOMES: [i32; 4] = [SNOWY_TUNDRA, SNOWY_TUNDRA, SNOWY_TUNDRA, SNOWY_TAIGA];

/// Shallow (non-deep) ocean ids. For this ruleset only `ocean` and `frozen_ocean`
/// occur.
#[inline]
pub fn is_shallow_ocean(id: i32) -> bool {
    id == OCEAN || id == FROZEN_OCEAN
}

/// Any ocean id (shallow or deep).
#[inline]
pub fn is_oceanic(id: i32) -> bool {
    id == OCEAN || id == FROZEN_OCEAN || id == DEEP_OCEAN
}

/// Category sentinel for biomes with no family.
pub const NONE: i32 = -1;

/// The family/category a biome belongs to (used by edge replacement). The return
/// value is a representative biome id for the family.
pub fn category(id: i32) -> i32 {
    match id {
        BEACH | SNOWY_BEACH => BEACH,
        DESERT | DESERT_HILLS | DESERT_LAKES => DESERT,
        MOUNTAINS
        | MOUNTAIN_EDGE
        | WOODED_MOUNTAINS
        | GRAVELLY_MOUNTAINS
        | MODIFIED_GRAVELLY_MOUNTAINS => MOUNTAINS,
        FOREST | WOODED_HILLS | BIRCH_FOREST | BIRCH_FOREST_HILLS | DARK_FOREST | FLOWER_FOREST
        | TALL_BIRCH_FOREST | TALL_BIRCH_HILLS | DARK_FOREST_HILLS => FOREST,
        SNOWY_TUNDRA | SNOWY_MOUNTAINS | ICE_SPIKES => SNOWY_TUNDRA,
        JUNGLE | JUNGLE_HILLS | JUNGLE_EDGE | MODIFIED_JUNGLE | MODIFIED_JUNGLE_EDGE => JUNGLE,
        // The badlands plateaus map to the badlands (mesa) family in this ruleset.
        BADLANDS
        | WOODED_BADLANDS_PLATEAU
        | BADLANDS_PLATEAU
        | ERODED_BADLANDS
        | MODIFIED_WOODED_BADLANDS_PLATEAU
        | MODIFIED_BADLANDS_PLATEAU => BADLANDS,
        MUSHROOM_FIELDS | MUSHROOM_FIELD_SHORE => MUSHROOM_FIELDS,
        STONE_SHORE => STONE_SHORE,
        OCEAN | FROZEN_OCEAN | DEEP_OCEAN => OCEAN,
        PLAINS | SUNFLOWER_PLAINS => PLAINS,
        RIVER | FROZEN_RIVER => RIVER,
        SAVANNA | SAVANNA_PLATEAU | SHATTERED_SAVANNA | SHATTERED_SAVANNA_PLATEAU => SAVANNA,
        SWAMP | SWAMP_HILLS => SWAMP,
        TAIGA
        | TAIGA_HILLS
        | SNOWY_TAIGA
        | SNOWY_TAIGA_HILLS
        | GIANT_TREE_TAIGA
        | GIANT_TREE_TAIGA_HILLS
        | TAIGA_MOUNTAINS
        | SNOWY_TAIGA_MOUNTAINS
        | GIANT_SPRUCE_TAIGA
        | GIANT_SPRUCE_TAIGA_HILLS => TAIGA,
        _ => NONE,
    }
}

/// Whether two biomes are "similar" (same family) for edge replacement: identical,
/// both badlands plateaus, or same [`category`].
pub fn are_similar(id1: i32, id2: i32) -> bool {
    if id1 == id2 {
        return true;
    }
    if id1 == WOODED_BADLANDS_PLATEAU || id1 == BADLANDS_PLATEAU {
        return id2 == WOODED_BADLANDS_PLATEAU || id2 == BADLANDS_PLATEAU;
    }
    category(id1) == category(id2)
}

/// Snowy/frozen biomes (used by the shore layer).
#[inline]
pub fn is_snowy(id: i32) -> bool {
    matches!(
        id,
        FROZEN_OCEAN
            | FROZEN_RIVER
            | SNOWY_TUNDRA
            | SNOWY_MOUNTAINS
            | SNOWY_BEACH
            | SNOWY_TAIGA
            | SNOWY_TAIGA_HILLS
    )
}
