use mod_api::calls;
use mod_api::{BlockId, WorldgenStage};
mod colony;
mod terrain;
pub use colony::{isqrt, smoothstep01, Colony, ColonyField};
pub use terrain::TerrainCache;

host_fn! {
    pub fn terrain_heights_at(columns: Vec<[i32; 2]>) -> Vec<i32>
        => TerrainHeightsAt { columns } => TerrainHeights
}

#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::host_fn;

host_fn! {
    pub fn register_worldgen_feature(stage: WorldgenStage, feature_id: u32, filter: mod_api::GenFeatureFilter)
        => RegisterWorldgenFeature { feature_id, stage, filter }
}

host_fn! {
    pub fn register_stage_replacement(stage: WorldgenStage, callback_id: u32)
        => RegisterStageReplacement { stage, callback_id }
}

host_fn! {
    pub fn register_generator(callback_id: u32) => RegisterGenerator { callback_id }
}

host_fn! {
    pub fn resolve_underground_biome(key: &str) -> Option<u8>
        => ResolveUndergroundBiome { key: key.into() } => MaybeByte
}

host_fn! {
    pub fn underground_biome_at(positions: Vec<[i32; 3]>) -> Vec<u8>
        => UndergroundBiomeAt { positions } => UndergroundBiomes
}

host_fn! {
    pub fn underground_biomes_in_box(lo: [i32; 3], hi: [i32; 3]) -> Vec<u8>
        => UndergroundBiomesInBox { lo, hi } => UndergroundBiomes
}

host_fn! {
    pub fn terrain_space_at(positions: Vec<[i32; 3]>) -> Vec<crate::TerrainSpace>
        => TerrainSpaceAt { positions } => TerrainSpaces
}

host_fn! {
    /// Whether generated TERRAIN is solid at each position, parallel to `positions` (max
    /// [`crate::SIM_BATCH_MAX`] per call). `false` means air or water.
    ///
    /// Pure function of (seed, position): the density surface minus the cave carve. Like
    /// [`underground_biome_at`], it answers before any section exists, so every section's
    /// dispatch gets the same answer.
    ///
    /// That's what lets a structure spanning sections make one accept/reject decision.
    /// [`GenCtx::block`] can't: it returns `None` outside the dispatching section, so some
    /// sections would accept the origin and others reject it, and the structure comes out
    /// fragmented. Use `block` for per-cell clipping (owner's cells only), this for the decision.
    ///
    /// Terrain only. Ore veins, vegetation, trees and other mods' writes aren't positional, so
    /// they aren't included. Same batching rule: query the handful of cells a decision needs,
    /// never a volume.
    pub fn terrain_solid_at(positions: Vec<[i32; 3]>) -> Vec<bool>
        => TerrainSolidAt { positions } => TerrainSolid
}

host_fn! {
    /// The final SURFACE biome of each world column `[x, z]`, parallel to
    /// `columns` (at most [`crate::SIM_BATCH_MAX`] per call). Ids are [`mod_api::biome`] names.
    ///
    /// The day-surface member of the same positional family, and it answers
    /// the two questions [`GenCtx::biome`] structurally cannot:
    ///
    /// - a decision about a structure that SPANS sections (the owner and its
    ///   neighbours must agree, and `GenCtx::biome` is `None` outside the
    ///   dispatching section, so they would not); and
    /// - anything about a NEIGHBOURING column — "is there a river within 4
    ///   blocks", which is what tells a river bank apart from ordinary plains,
    ///   and which no column knows about itself.
    ///
    /// Roll the cheap positional dice FIRST and batch only the survivors'
    /// anchors and probe offsets into one call per dispatch. A per-column
    /// query over a section is the shape that trips the mod watchdog.
    pub fn surface_biome_at(columns: Vec<[i32; 2]>) -> Vec<u8>
        => SurfaceBiomeAt { columns } => SurfaceBiomes
}

host_fn! {
    pub fn terrain_blocks_at(positions: Vec<[i32; 3]>) -> Vec<BlockId>
        => TerrainBlocksAt { positions } => BlockList
}

pub fn terrain_section_at(section: [i32; 3]) -> Vec<BlockId> {
    match crate::__rt::host_call(&crate::HostCall::from(calls::TerrainSectionAt { section })) {
        crate::HostRet::SectionBlocks(bytes) => bytes
            .chunks_exact(2)
            .map(|pair| BlockId(u16::from_le_bytes([pair[0], pair[1]])))
            .collect(),
        other => panic!("TerrainSectionAt returned {other:?}"),
    }
}

/// One worldgen dispatch's inputs, plus the accessors a well-behaved feature needs.
/// Read the seam/determinism contract below before writing one. The engine can't check
/// it for you, and a violation just shows up as features cut off at section borders.
///
/// # The worldgen determinism & seam contract
///
/// Sections generate independently, in any order, on any thread. The engine dispatches
/// your feature once per section and clips the returned writes to that section. So a
/// feature whose blocks span sections only comes out seamless if every section's call
/// re-derives the same decision for a shared origin. That holds automatically if a
/// per-origin decision uses only:
///
/// - positional RNG: [`GenRng::positional`] over `(ctx.seed(), your own salt,
///   origin coords)`, never a stateful stream, never state kept in `self`;
/// - column data ([`GenCtx::surface_y`], [`GenCtx::biome`], [`GenCtx::sea_level`]),
///   identical for every section of a column, so a column-anchored feature can span
///   any number of vertical sections;
/// - per-cell occupancy checks via [`GenCtx::block`], applied only to cells inside
///   the current section (cells outside return `None`; emit nothing for them, the
///   owning section's call handles its own cells).
///
/// Column data only covers this section's own 16x16 footprint. An origin in a
/// horizontal margin (a neighbouring column) has no surface/biome data here, so
/// reaching across columns is only safe for purely positional decisions (e.g.
/// underground blobs at absolute Y, iterated via [`GenCtx::for_each_origin`] with a
/// margin equal to the feature's horizontal reach). Surface-anchored features should
/// keep margin 0 and write only into the origin's own column.
///
/// [`underground_biome_at`] is also a pure function of position, so it's a legal
/// decision input on the same footing as [`GenRng::positional`], and a cross-section
/// feature can gate on "is this anchor inside my cave biome" and every section's call
/// will re-derive the same answer.
pub struct GenCtx {
    pub(crate) section_pos: [i32; 3],
    pub(crate) seed: u32,
    pub(crate) blocks: Vec<u16>,
    pub(crate) surface_heights: Vec<i32>,
    pub(crate) biomes: Vec<u8>,
    pub(crate) sea_level: i32,
}

impl GenCtx {
    pub fn for_test(
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u16>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    ) -> GenCtx {
        GenCtx {
            section_pos,
            seed,
            blocks,
            surface_heights,
            biomes,
            sea_level,
        }
    }

    pub fn section_pos(&self) -> [i32; 3] {
        self.section_pos
    }

    pub fn origin_world(&self) -> [i32; 3] {
        [
            self.section_pos[0] * 16,
            self.section_pos[1] * 16,
            self.section_pos[2] * 16,
        ]
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    pub fn surface_y(&self, wx: i32, wz: i32) -> Option<i32> {
        Some(self.surface_heights[self.column_index(wx, wz)?])
    }

    pub fn biome(&self, wx: i32, wz: i32) -> Option<u8> {
        Some(self.biomes[self.column_index(wx, wz)?])
    }

    pub fn biomes(&self) -> &[u8] {
        &self.biomes
    }

    pub fn has_block_snapshot(&self) -> bool {
        self.blocks.len() == 4096
    }

    pub fn block(&self, p: [i32; 3]) -> Option<BlockId> {
        if !self.has_block_snapshot() {
            return None;
        }
        let o = self.origin_world();
        let (lx, ly, lz) = (p[0] - o[0], p[1] - o[1], p[2] - o[2]);
        if !(0..16).contains(&lx) || !(0..16).contains(&ly) || !(0..16).contains(&lz) {
            return None;
        }
        Some(BlockId(
            self.blocks[(ly as usize) * 256 + (lz as usize) * 16 + lx as usize],
        ))
    }

    pub fn for_each_origin(&self, margin: i32, mut f: impl FnMut(i32, i32)) {
        let o = self.origin_world();
        for wz in (o[2] - margin)..(o[2] + 16 + margin) {
            for wx in (o[0] - margin)..(o[0] + 16 + margin) {
                f(wx, wz);
            }
        }
    }

    fn column_index(&self, wx: i32, wz: i32) -> Option<usize> {
        let o = self.origin_world();
        let (lx, lz) = (wx - o[0], wz - o[2]);
        if (0..16).contains(&lx) && (0..16).contains(&lz) && self.surface_heights.len() == 256 {
            Some((lz as usize) * 16 + lx as usize)
        } else {
            None
        }
    }
}

pub fn splitmix64_mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub struct GenRng {
    state: u64,
}

impl GenRng {
    pub const fn salt(name: &str) -> u64 {
        let bytes = name.as_bytes();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut i = 0;
        while i < bytes.len() {
            h = (h ^ bytes[i] as u64).wrapping_mul(0x0000_0100_0000_01b3);
            i += 1;
        }
        h
    }

    pub fn positional(seed: u32, salt: u64, wx: i32, wy: i32, wz: i32) -> Self {
        let z = splitmix64_mix(
            (seed as u64)
                ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (wx as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                ^ (wy as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
                ^ (wz as i64 as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93),
        );
        Self {
            state: if z == 0 { 0xDEAD_BEEF } else { z },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn next_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as i32
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}

#[cfg(test)]
mod tests {
    use super::GenRng;

    #[test]
    fn positional_stream_matches_the_engine_contract() {
        let mut rng = GenRng::positional(0x1234_5678, 0x0000_7a3e_0ac0_ffee, 12, 0, -34);
        assert_eq!(
            [
                rng.next_u64(),
                rng.next_u64(),
                rng.next_u64(),
                rng.next_u64()
            ],
            [
                0x6ac6_a985_c496_4f45,
                0x44e3_bbfd_0652_129b,
                0x75f9_7613_ca75_707e,
                0xa90a_c427_548e_451e,
            ],
        );
        let mut zero = GenRng::positional(0, 0, 0, 0, 0);
        assert_eq!(zero.next_u64(), 0x37c5_9ca7_bf06_be52);
    }

    #[test]
    fn named_salts_are_fnv1a() {
        const EMPTY: u64 = GenRng::salt("");
        assert_eq!(EMPTY, 0xcbf2_9ce4_8422_2325);
        assert_eq!(GenRng::salt("a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(GenRng::salt("mymod:a"), GenRng::salt("mymod:b"));
    }
}
