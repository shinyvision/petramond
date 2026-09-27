use crate::legality::prelude::*;
use crate::sched::WorldgenStage;

host_domain! {
    WorldgenCall {
        RegisterWorldgenFeature {
            feature_id: u32,
            stage: WorldgenStage,
            filter: crate::GenFeatureFilter,
        } => legal(SERVER_WORLDGEN, Init, Write),
        RegisterStageReplacement {
            stage: WorldgenStage,
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        RegisterGenerator {
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        ResolveUndergroundBiome {
            key: String,
        } => legal(EVERY, Any, Read),
        UndergroundBiomeAt {
            positions: Vec<[i32; 3]>,
        } => legal(BESIDE_WORLD, Any, Read),
        TerrainSolidAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        /// Which underground biomes CAN own a cell inside the inclusive world box
        /// `lo..=hi`? The bounded form of [`WorldgenCall::UndergroundBiomeAt`], and a
        /// REJECTION gate rather than an answer: the reply is a conservative
        /// superset (the engine bounds the partition field over the box's lattice
        /// cells instead of evaluating it per cell, and snaps the box outward to
        /// whole sections), so an id it OMITS provably owns nothing in the box,
        /// while an id it lists may still turn out absent.
        ///
        /// This is what makes a one-biome mod feature cheap: worldgen dispatches
        /// every registered feature for every section, and one box query over the
        /// dispatch's whole reach rejects the sections holding none of the mod's
        /// territory before any per-cell work is rolled or asked about.
        /// → [`HostRet::UndergroundBiomes`](crate::HostRet::UndergroundBiomes) (ascending ids).
        UndergroundBiomesInBox {
            lo: [i32; 3],
            hi: [i32; 3],
        } => legal(BESIDE_WORLD, Any, Read),
        SurfaceBiomeAt {
            columns: Vec<[i32; 2]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        TerrainSpaceAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        TerrainBlocksAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        TerrainHeightsAt {
            columns: Vec<[i32; 2]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        TerrainSectionAt {
            section: [i32; 3],
        } => legal(SERVER_WORLDGEN, Any, Read),
    }
}
