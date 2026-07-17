//! Worldgen hooks: block-id resolution, feature/stage/generator
//! registration, the per-dispatch [`GenCtx`], and the positional [`GenRng`]
//! mirroring the engine's frozen seeding contract.

use mod_api::{BlockId, WorldgenStage};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::host_fn;

host_fn! {
    /// Resolve a block registry key (`"petramond:stone"`, `"mymod:gadget"`) to its
    /// session-scoped runtime id. Works everywhere, worldgen instances included —
    /// resolve once in [`Mod::init`] and keep the id in mod state (but NEVER
    /// persist it: ids can change between sessions; names are the stable identity).
    pub fn resolve_block(key: &str) -> Option<BlockId> => ResolveBlock { key: key.into() } => Block
}

host_fn! {
    /// Every registered block carrying `tag`, in id order — engine tags as
    /// `"petramond:<name>"` (e.g. `"petramond:leaves"`), pack tags as their
    /// `"mod_id:name"`. Registry-only like [`resolve_block`]: works
    /// everywhere, any time; a name nothing lists is an empty set. Query once
    /// in [`Mod::init`] and keep the ids in mod state (never persist them) —
    /// tag-driven policy picks up pack-added blocks with no code change.
    pub fn blocks_by_tag(tag: &str) -> Vec<BlockId>
        => BlocksByTag { tag: tag.into() } => BlockList
}

/// [`resolve_block`] that also logs a "not registered" line on `None` — the
/// standard init-time shape: resolution failure is worth one log line, then
/// the mod degrades on the `None`.
pub fn resolve_block_logged(key: &str) -> Option<BlockId> {
    let id = resolve_block(key);
    if id.is_none() {
        crate::log(&format!("block '{key}' is not registered"));
    }
    id
}

host_fn! {
    /// Register a worldgen feature that runs after `stage` (use
    /// [`WorldgenStage::Trees`] — the end of the pipeline — unless the feature
    /// must see pre-vegetation ground). Only legal during [`Mod::init`];
    /// `Climate` is not a valid attach point. `feature_id` is echoed to
    /// [`Mod::gen_feature`].
    pub fn register_worldgen_feature(stage: WorldgenStage, feature_id: u32)
        => RegisterWorldgenFeature { feature_id, stage }
}

host_fn! {
    /// Replace one engine worldgen stage. Only legal during [`Mod::init`];
    /// `callback_id` is echoed to [`Mod::gen_climate`] / [`Mod::gen_terrain`] /
    /// [`Mod::gen_stage`] depending on the stage. Last mod in load order wins a
    /// conflict; a failing replacement falls back to the engine stage.
    pub fn register_stage_replacement(stage: WorldgenStage, callback_id: u32)
        => RegisterStageReplacement { stage, callback_id }
}

host_fn! {
    /// Replace the whole generator: every stage dispatches to `callback_id` (your
    /// `gen_climate`/`gen_terrain`/`gen_stage` switch on the stage). Same rules as
    /// [`register_stage_replacement`], applied per stage.
    pub fn register_generator(callback_id: u32) => RegisterGenerator { callback_id }
}

/// One worldgen dispatch's inputs, with the accessors a well-behaved feature
/// needs. See the seam/determinism contract below — the engine cannot check it
/// for you; a violation shows up as features cut off at section borders.
///
/// # The worldgen determinism & seam contract
///
/// Sections generate independently, in any order, on any thread. The engine
/// dispatches your feature once per section and CLIPS the returned writes to
/// that section. A feature whose blocks span sections therefore only comes out
/// seamless if every section's call re-derives the SAME decisions for a shared
/// origin. That holds automatically when a per-origin decision uses only:
///
/// - positional RNG: [`GenRng::positional`] over `(ctx.seed(), your own salt,
///   origin coords)` — never a stateful stream, never state kept in `self`;
/// - the column data ([`GenCtx::surface_y`], [`GenCtx::biome`],
///   [`GenCtx::sea_level`]), which is IDENTICAL for every section of a column
///   — so a column-anchored feature may span any number of VERTICAL sections;
/// - per-cell occupancy predicates via [`GenCtx::block`] applied only to cells
///   inside the current section (out-of-section cells return `None`; emit
///   nothing for them — the owning section's call emits its own cells).
///
/// Column data covers only this section's own 16×16 footprint. An origin in a
/// HORIZONTAL margin (a neighbouring column) has no surface/biome data here,
/// so cross-column reach is safe only for decisions that are purely positional
/// (e.g. underground blobs at absolute Y, iterated via
/// [`GenCtx::for_each_origin`] with a margin equal to the feature's horizontal
/// reach). Surface-anchored features should keep margin 0 and write only in
/// the origin's own column.
pub struct GenCtx {
    pub(crate) section_pos: [i32; 3],
    pub(crate) seed: u32,
    pub(crate) blocks: Vec<u8>,
    pub(crate) surface_heights: Vec<i32>,
    pub(crate) biomes: Vec<u8>,
    pub(crate) sea_level: i32,
}

impl GenCtx {
    /// Section coordinates (16³ units).
    pub fn section_pos(&self) -> [i32; 3] {
        self.section_pos
    }

    /// The section's world origin (minimum corner).
    pub fn origin_world(&self) -> [i32; 3] {
        [
            self.section_pos[0] * 16,
            self.section_pos[1] * 16,
            self.section_pos[2] * 16,
        ]
    }

    /// The world seed — feed it to [`GenRng::positional`].
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Sea level (world Y of the waterline).
    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// The column's post-cave bare-ground surface (world Y, before
    /// vegetation/trees) at world `(wx, wz)`, or `None` outside this section's
    /// 16×16 footprint. Below [`GenCtx::sea_level`] = submerged or floorless.
    /// Identical for every section of the column.
    pub fn surface_y(&self, wx: i32, wz: i32) -> Option<i32> {
        Some(self.surface_heights[self.column_index(wx, wz)?])
    }

    /// The biome id at world `(wx, wz)`, or `None` outside the footprint.
    /// Identical for every section of the column.
    pub fn biome(&self, wx: i32, wz: i32) -> Option<u8> {
        Some(self.biomes[self.column_index(wx, wz)?])
    }

    /// The engine's proposed biome map (`z*16 + x`) — only meaningful inside
    /// [`Mod::gen_climate`], where it is the map you are replacing.
    pub fn biomes(&self) -> &[u8] {
        &self.biomes
    }

    /// The block currently at world `p`, or `None` when `p` is outside this
    /// section (or the call carries no snapshot: `Climate`/`Terrain` stages).
    /// Use it for per-cell occupancy predicates ("only place over air") on the
    /// cells you emit — each section checks exactly the cells it owns.
    pub fn block(&self, p: [i32; 3]) -> Option<BlockId> {
        if self.blocks.len() != 4096 {
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

    /// Iterate candidate feature origins over this section's XZ footprint plus
    /// `margin` extra columns on every side, in the engine's canonical
    /// `(wz, wx)` order — the same loop the engine's own features use. Use
    /// margin 0 for column-anchored features; a positive margin only for
    /// purely positional ones (see the contract on [`GenCtx`]).
    pub fn for_each_origin(&self, margin: i32, mut f: impl FnMut(i32, i32)) {
        let o = self.origin_world();
        for wz in (o[2] - margin)..(o[2] + 16 + margin) {
            for wx in (o[0] - margin)..(o[0] + 16 + margin) {
                f(wx, wz);
            }
        }
    }

    /// `z*16 + x` index for a world column inside the footprint.
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

/// The SplitMix64 finalizer — the engine's frozen bit-mixing primitive (the
/// `entity::hash01` finalizer, the [`GenRng::positional`] seed mix). Use it to
/// spread correlated inputs (one shared RNG draw XOR a stable id, packed
/// coordinates) into decorrelated u64s without a host call per input.
pub fn splitmix64_mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic positional RNG for worldgen hooks — the guest-side mirror of
/// the engine's frozen positional seeding contract (same SplitMix64 finalizer,
/// same xorshift64 stepper), so mod features get engine-grade order
/// independence by default. Derive every independent stream from
/// `(world seed, your own salt, world coords)`; NEVER carry RNG state between
/// dispatches. Pick a salt unique to your mod/feature (any constant — hash
/// your feature name) so your stream is decorrelated from the engine's and
/// from other mods'.
pub struct GenRng {
    state: u64,
}

impl GenRng {
    /// Seed from `(seed, salt, world coords)` — a pure function of the inputs,
    /// bit-identical across platforms.
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

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform integer in `[lo, hi]` (inclusive).
    pub fn next_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as i32
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}

#[cfg(test)]
mod tests {
    use super::GenRng;

    /// [`GenRng`] mirrors the ENGINE's frozen positional seeding contract
    /// (`src/worldgen/rng.rs` pins the same vectors) — if this drifts, mod
    /// features lose engine-grade determinism. Never "fix" these numbers;
    /// fix the generator.
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
}
