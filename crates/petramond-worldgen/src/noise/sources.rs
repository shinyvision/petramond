use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::cave_density::CaveDensity;
use crate::density::terrain::{TerrainDensityGraph, TerrainDensitySpec};

#[derive(Clone)]
pub(crate) struct SeedSources {
    pub(crate) terrain: Arc<TerrainDensityGraph>,
    pub(super) natural: Arc<CaveDensity>,
}

type SourceKey = (u32, u64);

struct Entry {
    key: SourceKey,
    terrain: Weak<TerrainDensityGraph>,
    natural: Weak<CaveDensity>,
}

impl SeedSources {
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
