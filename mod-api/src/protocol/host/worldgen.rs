//! Worldgen hooks and the pure positional terrain queries: gen registrations,
//! the underground-biome partition, terrain columns, sections and spaces — pure
//! functions of (world seed, position), legal on the detached worldgen
//! instances.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::legality::prelude::*;
use crate::sched::WorldgenStage;

host_domain! {
    /// Worldgen hooks and the pure positional terrain queries: gen registrations,
    /// the underground-biome partition, terrain columns, sections and spaces — pure
    /// functions of (world seed, position), legal on the detached worldgen
    /// instances.
    WorldgenCall {
        /// Register a worldgen FEATURE that runs after `stage` (typically
        /// [`WorldgenStage::Trees`], the end of the pipeline). Legal ONLY during
        /// `mod_init`; `stage == Climate` is rejected (features write blocks;
        /// climate is column-level). The engine later dispatches
        /// [`GuestCall::GenFeature`](crate::GuestCall::GenFeature) once per generated 16³ section, on worldgen
        /// worker threads — see the determinism contract on [`GuestCall::GenFeature`](crate::GuestCall::GenFeature).
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        RegisterWorldgenFeature {
            feature_id: u32,
            stage: WorldgenStage,
            filter: crate::GenFeatureFilter,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// REPLACE one engine worldgen stage. Legal ONLY during `mod_init`. The
        /// engine dispatches [`GuestCall::GenStage`](crate::GuestCall::GenStage) instead of running its own
        /// stage. If several mods replace the same stage, the LAST in load order
        /// wins (logged). A failing replacement falls back to the ENGINE stage.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        RegisterStageReplacement {
            stage: WorldgenStage,
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// Replace the WHOLE generator: shorthand for replacing every stage with
        /// `callback_id` (the guest switches on the dispatched `stage`). Same
        /// window, conflict, and fallback rules as
        /// [`WorldgenCall::RegisterStageReplacement`]. → [`HostRet::Unit`](crate::HostRet::Unit).
        RegisterGenerator {
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// Resolve an UNDERGROUND-BIOME registry name (`"petramond:marble"`,
        /// `"mymod:mushroom_cavern"` — the `underground_biome` field of an
        /// `underground_biomes.json` row) to its session-scoped id. The twin of
        /// [`RegistryCall::ResolveShape`]: registry-only, legal on ANY instance, any
        /// time (worldgen instances included). `None` = no such row. Resolve once
        /// in `Mod::init` and keep the id; NEVER persist it — underground-biome
        /// ids are session-scoped and are never written to disk or the wire.
        /// → [`HostRet::MaybeByte`](crate::HostRet::MaybeByte).
        ResolveUndergroundBiome {
            key: String,
        } => legal(EVERY, Any, Read),
        /// The underground biome owning each world cell, reply parallel to
        /// `positions`. A pure function of (world seed, position) reading the
        /// SAME world-anchored lattice the cave carver reads — so it answers
        /// during worldgen, before any section exists, and it agrees with the
        /// wall lining and cave caliber at that cell by construction. Total: a
        /// cell no row claims is the fallback row, id 0 — never `None`, never an
        /// unloaded answer. Bounded batch (4096 positions per call).
        /// → [`HostRet::UndergroundBiomes`](crate::HostRet::UndergroundBiomes).
        UndergroundBiomeAt {
            positions: Vec<[i32; 3]>,
        } => legal(BESIDE_WORLD, Any, Read),
        /// Is the GENERATED TERRAIN solid at each world cell, reply parallel to
        /// `positions`? A pure function of (world seed, position): the density
        /// surface minus the cave carve — the same two decisions the engine's own
        /// fill and carve make — so it answers during worldgen, before any section
        /// exists, and every section's dispatch gets the same answer for a shared
        /// cell. `false` = air or water; features (ores, vegetation, mod writes)
        /// are NOT included. Bounded batch (4096 positions per call).
        /// → [`HostRet::TerrainSolid`](crate::HostRet::TerrainSolid).
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
        /// The final SURFACE biome of each world column, reply parallel to
        /// `columns` (`[x, z]`). The day-surface member of the positional
        /// worldgen family, and subject to the same rules as
        /// [`TerrainSolidAt`](Self::TerrainSolidAt): a pure function of (world
        /// seed, column) read off the same world-anchored tile the feature stage
        /// itself reads, so it answers on a detached worldgen instance with no
        /// section loaded and agrees with [`GenCtx`'s own column
        /// map](crate::GuestCall::GenFeature) by construction. Ids are
        /// [`crate::biome`] names; there is no "unknown" — every column has a
        /// biome. Bounded batch (4096 columns per call).
        ///
        /// It exists because a feature's own column data covers ONLY the
        /// dispatching section's 16×16, so it cannot gate anything that spans
        /// sections or reads a NEIGHBOURING column: a structure whose owner
        /// checks the biome would be accepted in one section and rejected in the
        /// next, and "is there a river within N blocks" — the question that
        /// decides a river bank — is not a question one column knows the answer
        /// to at all. Query ANCHORS and probe offsets, a handful per section,
        /// never a volume.
        /// → [`HostRet::SurfaceBiomes`](crate::HostRet::SurfaceBiomes).
        SurfaceBiomeAt {
            columns: Vec<[i32; 2]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        /// Positional terrain occupancy, before features; at most SIM_BATCH_MAX
        /// positions, in request order. Legal on detached generation instances.
        TerrainSpaceAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        /// Filled and carved terrain materials, before feature stages; reply in request order.
        TerrainBlocksAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        /// Highest solid density cells before cave carving and feature stages.
        TerrainHeightsAt {
            columns: Vec<[i32; 2]>,
        } => legal(SERVER_WORLDGEN, Any, Read),
        /// [`WorldgenCall::TerrainBlocksAt`] for one whole 16³ section in section
        /// order (`(y * 16 + z) * 16 + x`): what a tile-caching reader asks,
        /// without shipping 4,096 positions to say so.
        TerrainSectionAt {
            section: [i32; 3],
        } => legal(SERVER_WORLDGEN, Any, Read),
    }
}
