//! The seed-derived generation sources — the terrain density graph (seeded
//! double-Perlin fields and splines) and the natural cave density (its twenty
//! noises) — built ONCE per world and shared by `Arc`.
//!
//! Both are immutable pure functions of `(seed, terrain recipe)`, and both
//! are expensive to build. The surface density system, the cave field and
//! every `ChunkGenerator` (one per worker thread, plus the shared one) used
//! to build their own copies; they now take handles from here, so a
//! generator is cheap to construct on any thread, sync paths included.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::cave_density::CaveDensity;
use crate::density::terrain::{TerrainDensityGraph, TerrainDensitySpec};

/// Handles to one world's seed-derived sources.
#[derive(Clone)]
pub(crate) struct SeedSources {
    /// The terrain density graph over the installed terrain recipe.
    pub(crate) terrain: Arc<TerrainDensityGraph>,
    /// The natural cave density.
    pub(super) natural: Arc<CaveDensity>,
}

/// What a set of sources is built from: the seed and the terrain recipe's
/// fingerprint.
type SourceKey = (u32, u64);

/// A built set, held weakly: sources live exactly as long as some generator
/// uses them, so the registry never pins a finished world's noise.
struct Entry {
    key: SourceKey,
    terrain: Weak<TerrainDensityGraph>,
    natural: Weak<CaveDensity>,
}

impl SeedSources {
    /// The sources for `seed` under the installed terrain recipe: the live
    /// set when any holder still has one, else a fresh build that every
    /// later caller shares. Concurrent first callers wait for the one build
    /// instead of each paying for it.
    pub(crate) fn for_seed(seed: u32) -> Self {
        static BUILT: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
        let key = (seed, crate::data::terrain::recipe().fingerprint);
        let mut built = BUILT.lock().unwrap_or_else(PoisonError::into_inner);
        let live = built.iter().find(|e| e.key == key).and_then(|e| {
            Some(Self {
                terrain: e.terrain.upgrade()?,
                natural: e.natural.upgrade()?,
            })
        });
        if let Some(sources) = live {
            return sources;
        }
        built.retain(|e| e.key != key && e.terrain.strong_count() > 0);
        let sources = Self {
            terrain: Arc::new(TerrainDensitySpec::default_surface().build_graph(seed)),
            natural: Arc::new(CaveDensity::new(seed)),
        };
        built.push(Entry {
            key,
            terrain: Arc::downgrade(&sources.terrain),
            natural: Arc::downgrade(&sources.natural),
        });
        sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every caller for a seed shares one build while it is alive; another
    /// seed gets its own.
    #[test]
    fn a_seed_builds_its_sources_once() {
        let a = SeedSources::for_seed(0x5005_0001);
        let b = SeedSources::for_seed(0x5005_0001);
        assert!(Arc::ptr_eq(&a.terrain, &b.terrain));
        assert!(Arc::ptr_eq(&a.natural, &b.natural));
        let other = SeedSources::for_seed(0x5005_0002);
        assert!(!Arc::ptr_eq(&a.terrain, &other.terrain));
    }
}
