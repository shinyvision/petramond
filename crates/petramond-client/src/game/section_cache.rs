use std::sync::Arc;

use rustc_hash::FxHashMap;

use petramond::net::protocol::{NameTables, SectionCacheClaim, SECTION_CACHE_CAP};
use petramond_world::chunk::SectionPos;
use petramond_world::section::Section;

pub fn section_cache_registry_key(tables: &NameTables) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    tables.blocks.hash(&mut h);
    petramond::net::remap::local_name_tables()
        .blocks
        .hash(&mut h);
    h.finish()
}

struct CachedSection {
    section: Arc<Section>,
    hash: u64,
    stamp: u64,
}

#[derive(Default)]
pub struct SectionCache {
    entries: FxHashMap<SectionPos, CachedSection>,
    next_stamp: u64,
    registry_key: Option<u64>,
}

impl SectionCache {
    pub fn park(&mut self, pos: SectionPos, section: Arc<Section>, hash: u64) {
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        self.entries.insert(
            pos,
            CachedSection {
                section,
                hash,
                stamp,
            },
        );
        if self.entries.len() > SECTION_CACHE_CAP {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.stamp)
                .map(|(p, _)| *p)
            {
                self.entries.remove(&oldest);
            }
        }
    }

    pub fn promote(&mut self, pos: SectionPos, hash: u64) -> Option<Arc<Section>> {
        let entry = self.entries.remove(&pos)?;
        (entry.hash == hash).then_some(entry.section)
    }

    pub fn discard(&mut self, pos: SectionPos) {
        self.entries.remove(&pos);
    }

    pub fn claims(&self) -> Vec<SectionCacheClaim> {
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_unstable_by_key(|(_, e)| e.stamp);
        entries
            .into_iter()
            .map(|(pos, e)| SectionCacheClaim {
                pos: *pos,
                hash: e.hash,
            })
            .collect()
    }

    pub fn adopt_session(&mut self, registry_key: u64) {
        if self.registry_key != Some(registry_key) {
            self.entries.clear();
        }
        self.registry_key = Some(registry_key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    pub fn contains(&self, pos: SectionPos) -> bool {
        self.entries.contains_key(&pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_section(pos: SectionPos) -> Arc<Section> {
        Arc::new(Section::new(pos.cx, pos.cy, pos.cz))
    }

    #[test]
    fn promote_returns_only_hash_matched_entries_once() {
        let mut cache = SectionCache::default();
        let pos = SectionPos::new(1, 2, 3);
        cache.park(pos, empty_section(pos), 77);
        assert!(cache.promote(pos, 99).is_none(), "hash mismatch = miss");
        assert!(
            cache.promote(pos, 77).is_none(),
            "a mismatched entry is dropped, not retried"
        );
        cache.park(pos, empty_section(pos), 77);
        assert!(cache.promote(pos, 77).is_some());
        assert!(cache.promote(pos, 77).is_none(), "promotion consumes");
    }

    #[test]
    fn cap_evicts_oldest_first() {
        let mut cache = SectionCache::default();
        for i in 0..=SECTION_CACHE_CAP {
            let pos = SectionPos::new(i as i32, 0, 0);
            cache.park(pos, empty_section(pos), i as u64);
        }
        assert_eq!(cache.len(), SECTION_CACHE_CAP);
        assert!(
            cache.promote(SectionPos::new(0, 0, 0), 0).is_none(),
            "the oldest entry fell out"
        );
        assert!(cache.promote(SectionPos::new(1, 0, 0), 1).is_some());
    }

    #[test]
    fn adopting_a_different_session_vocabulary_clears() {
        let mut cache = SectionCache::default();
        let pos = SectionPos::new(1, 0, 0);
        cache.park(pos, empty_section(pos), 5);
        cache.adopt_session(10);
        assert_eq!(cache.len(), 0, "unbound cache never survives adoption");
        cache.park(pos, empty_section(pos), 5);
        cache.adopt_session(10);
        assert_eq!(cache.len(), 1, "same vocabulary keeps entries");
        cache.adopt_session(11);
        assert_eq!(cache.len(), 0, "moved vocabulary clears");
    }
}
