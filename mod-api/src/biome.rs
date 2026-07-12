//! The stable biome vocabulary for mods.
//!
//! Worldgen hooks receive biomes as raw `u8` ids (see
//! [`GuestCall::GenFeature`](crate::GuestCall::GenFeature)). Unlike block/item
//! ids, biome ids are a COMPILED APPEND-ONLY table (they serialize into chunk
//! bytes), so the ids themselves are stable across sessions and saves and may
//! be named as constants. Mods must still address biomes through these names —
//! never copied numeric literals; the engine pins this table against its own
//! biome registry, so a drift fails an engine test, not a mod at runtime.
//!
//! When biomes become data-driven (explicitly future work), pack biomes will
//! get a name-resolution call like `ResolveBlock`; this vocabulary stays valid
//! for the engine rows.

/// Engine biome names, indexed by `id - 1` (biome ids start at 1; 0 is
/// unused). Append-only, mirroring the engine's biome table.
pub const BIOME_NAMES: &[&str] = &[
    "ocean",
    "beach",
    "river",
    "desert",
    "plains",
    "savanna",
    "forest",
    "swamp",
    "taiga",
    "snowy_tundra",
    "snowy_taiga",
    "mountains",
    "snowy_peaks",
    "deep_ocean",
    "foothills",
    "wetland",
    "redwood_forest",
    "old_growth_taiga",
    "cherry_grove",
    "meadow",
    "grove",
    "snowy_slopes",
    "windswept_hills",
    "stony_peaks",
    "wooded_hills",
    "mountain_edge",
    "desert_lakes",
];

pub const OCEAN: u8 = 1;
pub const BEACH: u8 = 2;
pub const RIVER: u8 = 3;
pub const DESERT: u8 = 4;
pub const PLAINS: u8 = 5;
pub const SAVANNA: u8 = 6;
pub const FOREST: u8 = 7;
pub const SWAMP: u8 = 8;
pub const TAIGA: u8 = 9;
pub const SNOWY_TUNDRA: u8 = 10;
pub const SNOWY_TAIGA: u8 = 11;
pub const MOUNTAINS: u8 = 12;
pub const SNOWY_PEAKS: u8 = 13;
pub const DEEP_OCEAN: u8 = 14;
pub const FOOTHILLS: u8 = 15;
pub const WETLAND: u8 = 16;
pub const REDWOOD_FOREST: u8 = 17;
pub const OLD_GROWTH_TAIGA: u8 = 18;
pub const CHERRY_GROVE: u8 = 19;
pub const MEADOW: u8 = 20;
pub const GROVE: u8 = 21;
pub const SNOWY_SLOPES: u8 = 22;
pub const WINDSWEPT_HILLS: u8 = 23;
pub const STONY_PEAKS: u8 = 24;
pub const WOODED_HILLS: u8 = 25;
pub const MOUNTAIN_EDGE: u8 = 26;
pub const DESERT_LAKES: u8 = 27;

/// The stable snake_case name for a biome id, `None` for an unknown id.
pub fn name(id: u8) -> Option<&'static str> {
    (id >= 1)
        .then(|| BIOME_NAMES.get(id as usize - 1).copied())
        .flatten()
}

/// Resolve a stable biome name to its id, `None` for an unknown name — for
/// mods whose biome choices are data (config strings) rather than constants.
pub fn by_name(name: &str) -> Option<u8> {
    BIOME_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| i as u8 + 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_and_constants_agree_both_ways() {
        assert_eq!(super::name(super::PLAINS), Some("plains"));
        assert_eq!(super::by_name("desert_lakes"), Some(super::DESERT_LAKES));
        assert_eq!(super::name(0), None);
        assert_eq!(super::by_name("nope"), None);
        for (i, n) in super::BIOME_NAMES.iter().enumerate() {
            assert_eq!(super::by_name(n), Some(i as u8 + 1));
        }
    }
}
