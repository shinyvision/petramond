//! The cave field's memos: positional cave facts every generator of a world
//! shares, declared in one place with their capacities (see `crate::cache`).

use std::sync::Arc;

use super::fluid_falls::ChunkFalls;
use super::CaveLattice;
use crate::cache::{inline, memo_group, pointee, slice, CacheBudget, MemoStats};
use crate::data::underground::ClimatePoint;
use crate::noise::cave_density::Sample;
use crate::noise::cave_walk::WalkMemos;
use crate::noise::chamber::{CandidateCache, ChamberField};

memo_group! {
    /// Memos over the cave field's own sources and derivations.
    pub(crate) struct CaveMemos {
        /// Completed cave-source lattice corners.
        source: super::source::Key => Sample =
            ("cave.source", 262_144, Frontier, inline),
        /// The climate channels of one cave lattice column.
        climate_columns: (u32, i32, i32) => ClimatePoint =
            ("cave.climate_columns", 32_768, Frontier, inline),
        /// Openness answers to repeated positional query batches.
        carved: super::query_cache::Key<Box<[super::query_cache::Query]>> => Vec<bool> =
            ("cave.carved_queries", 512, Fixed, |v: &Vec<bool>| v.capacity()),
        /// Fill answers to repeated positional query batches.
        filled: super::query_cache::Key<Box<[super::query_cache::Query]>> => Vec<Option<u16>> =
            ("cave.filled_queries", 512, Fixed, |v: &Vec<Option<u16>>| v.capacity() * 4),
        /// A 16×16 chunk's density surfaces (before caves and features): the
        /// cave model's sky line, and the terrain height queries' answer.
        density_surfaces: (u32, [i32; 2]) => Arc<[i32]> =
            ("cave.density_surfaces", 4096, Frontier, slice),
        /// The fluid falls sourced in one chunk.
        falls: super::fluid_falls::Key => Arc<ChunkFalls> =
            ("cave.falls", 4096, Frontier, pointee),
        /// Excavation terms gathered per padded 64-block column cell.
        chamber_fields: super::sampling::ChamberKey => Arc<ChamberField> =
            ("cave.chamber_fields", 1024, Frontier, pointee),
        /// Quart-resolution habitat candidate columns per 16×16 tile.
        regions: super::regions::Key => Arc<super::regions::Tile> =
            ("cave.regions", 16_384, Frontier, pointee),
        /// Conservative habitat id sets per territory grid.
        territory: super::territory::Key => Arc<super::territory::Grid> =
            ("cave.territory", 8192, Frontier, pointee),
        /// Excavation placement sites per spacing cell.
        volume_sites: super::volumes::SiteKey => Option<super::volumes::Site> =
            ("cave.volume_sites", 16_384, Frontier, inline),
        /// Excavation voxel tiles (16³).
        volume_tiles: super::volumes::TileKey => Arc<super::volumes::Tile> =
            ("cave.volume_tiles", 4096, Frontier, pointee),
        /// Excavation biome claims per 16×16 column.
        claims: super::volumes::claims::Key => super::volumes::claims::Claims =
            ("cave.claims", 8192, Frontier, slice),
        /// A projection rule's cut plane across a chunk at one height.
        planes: super::volumes::PlaneKey => Arc<[u16; 256]> =
            ("cave.projection_planes", 2048, Frontier, pointee),
        /// A projection rule's anchors across a chunk.
        anchors: super::volumes::AnchorKey => super::volumes::ChunkAnchors =
            ("cave.projection_anchors", 4096, Frontier, pointee),
        /// The fluid pool rolled in one pool cell (most cells roll none).
        pools: super::fluid_pools::PoolKey => Option<Arc<super::fluid_pools::Pool>> =
            ("cave.pools", 262_144, Frontier, |p: &Option<Arc<super::fluid_pools::Pool>>| {
                p.as_ref().map_or(0, pointee)
            }),
        /// The cave lattice over one pool cell, shared by neighbouring floods.
        pool_tiles: super::fluid_pools::TileKey => Arc<CaveLattice> =
            ("cave.pool_tiles", 2048, Frontier, pointee),
    }
}

/// Every cave memo of a world: the field's own, the branching walks', and the
/// excavation room admissions.
pub(crate) struct CaveCaches {
    pub(super) memos: CaveMemos,
    pub(super) walks: WalkMemos,
    pub(super) candidates: CandidateCache,
}

impl CaveCaches {
    pub(crate) fn new(budget: CacheBudget) -> Self {
        Self {
            memos: CaveMemos::new(budget),
            walks: WalkMemos::new(budget),
            candidates: CandidateCache::new(budget),
        }
    }

    pub(crate) fn stats(&self) -> Vec<MemoStats> {
        let mut out = self.memos.stats();
        out.extend(self.walks.stats());
        out.push(self.candidates.stats());
        out
    }

    pub(crate) fn clear(&self) {
        self.memos.clear();
        self.walks.clear();
        self.candidates.clear();
    }
}
