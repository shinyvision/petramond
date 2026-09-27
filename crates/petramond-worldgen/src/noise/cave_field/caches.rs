use std::sync::Arc;

use super::fluid_falls::ChunkFalls;
use super::CaveLattice;
use crate::cache::{inline, memo_group, pointee, slice, CacheBudget, GenContext, MemoStats};
use crate::data::underground::ClimatePoint;
use crate::noise::cave_density::Sample;
use crate::noise::cave_walk::WalkMemos;
use crate::noise::chamber::{CandidateCache, ChamberField};

memo_group! {
    pub(crate) struct CaveMemos {
        source: super::source::Key => Sample =
            ("cave.source", 262_144, Frontier, inline),
        climate_columns: (GenContext, [i32; 2]) => ClimatePoint =
            ("cave.climate_columns", 32_768, Frontier, inline),
        carved: super::query_cache::Key<Box<[super::query_cache::Query]>> => Vec<bool> =
            ("cave.carved_queries", 512, Fixed, |v: &Vec<bool>| v.capacity()),
        filled: super::query_cache::Key<Box<[super::query_cache::Query]>> => Vec<Option<u16>> =
            ("cave.filled_queries", 512, Fixed, |v: &Vec<Option<u16>>| v.capacity() * 4),
        density_surfaces: (GenContext, [i32; 2]) => Arc<[i32]> =
            ("cave.density_surfaces", 4096, Frontier, slice),
        falls: super::fluid_falls::Key => Arc<ChunkFalls> =
            ("cave.falls", 4096, Frontier, pointee),
        chamber_fields: super::sampling::ChamberKey => Arc<ChamberField> =
            ("cave.chamber_fields", 1024, Frontier, pointee),
        regions: super::regions::Key => Arc<super::regions::Tile> =
            ("cave.regions", 16_384, Frontier, pointee),
        territory: super::territory::Key => Arc<super::territory::Grid> =
            ("cave.territory", 8192, Frontier, pointee),
        volume_sites: super::volumes::SiteKey => Option<super::volumes::Site> =
            ("cave.volume_sites", 16_384, Frontier, inline),
        volume_tiles: super::volumes::TileKey => Arc<super::volumes::Tile> =
            ("cave.volume_tiles", 4096, Frontier, pointee),
        claims: super::volumes::claims::Key => super::volumes::claims::Claims =
            ("cave.claims", 8192, Frontier, slice),
        planes: super::volumes::PlaneKey => Arc<[u16; 256]> =
            ("cave.projection_planes", 2048, Frontier, pointee),
        anchors: super::volumes::AnchorKey => super::volumes::ChunkAnchors =
            ("cave.projection_anchors", 4096, Frontier, pointee),
        pools: super::fluid_pools::PoolKey => Option<Arc<super::fluid_pools::Pool>> =
            ("cave.pools", 262_144, Frontier, |p: &Option<Arc<super::fluid_pools::Pool>>| {
                p.as_ref().map_or(0, pointee)
            }),
        pool_tiles: super::fluid_pools::TileKey => Arc<CaveLattice> =
            ("cave.pool_tiles", 2048, Frontier, pointee),
    }
}

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
