//! Layered worldgen content catalogs: the rows a pack layers over, parsed
//! into the tables generation reads — including the generation rules
//! (`biome_gen`) and tree placement (`tree_profiles`) of each biome row.

pub mod biome_gen;
pub mod bounds;
pub mod excavations;
pub mod features;
pub mod tree_profiles;
pub mod underground;
