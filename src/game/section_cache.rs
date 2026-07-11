//! The client-side section cache (WIKI/section-cache.md): replica sections
//! evicted by the server's keep-shape unloads, parked under the SERVER-DOMAIN
//! content hash the unload vouched, and re-promoted by
//! [`SectionCached`](crate::net::protocol::ServerToClient::SectionCached)
//! without re-streaming or re-decoding the payload.
//!
//! The cache is IN-MEMORY ONLY. Cached blocks are CLIENT-LOCAL ids (the
//! transport remapped them on ingest), so entries are meaningful exactly as
//! long as this process interprets those ids the same way — [`Self::
//! adopt_session`] guards that boundary across sessions and NOTHING here may
//! ever be persisted.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::chunk::SectionPos;
use crate::net::protocol::{NameTables, SectionCacheClaim, SECTION_CACHE_CAP};
use crate::section::Section;

/// Fingerprint of a remote session's block-id vocabulary: the server's block
/// name table (wire-id order) plus this client's own — together they define
/// what every cached client-local block id means. Only blocks matter here:
/// sections carry block ids (block buffer, slab layers) while every other
/// payload field is index- or name-addressed.
pub(crate) fn section_cache_registry_key(tables: &NameTables) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    tables.blocks.hash(&mut h);
    crate::net::remap::local_name_tables().blocks.hash(&mut h);
    h.finish()
}

struct CachedSection {
    section: Arc<Section>,
    /// The server-domain content hash vouched at unload — echoed in
    /// [`SectionCacheClaim`]s and checked against `SectionCached::hash`.
    hash: u64,
    /// Insertion stamp for oldest-first eviction (see `SECTION_CACHE_CAP`).
    stamp: u64,
}

/// Parked evicted sections, keyed by position, capped at
/// [`SECTION_CACHE_CAP`] with oldest-first eviction — the same policy the
/// server's per-connection belief map runs, so the two stay aligned without
/// eviction chatter (unloads arrive in the order the server issued them).
#[derive(Default)]
pub(crate) struct SectionCache {
    entries: FxHashMap<SectionPos, CachedSection>,
    next_stamp: u64,
    /// Fingerprint of the id vocabulary the cached sections were built under
    /// (server block name table + this client's). `None` until a session
    /// adopts the cache.
    registry_key: Option<u64>,
}

impl SectionCache {
    /// Park one evicted section under the server-vouched content hash.
    pub(crate) fn park(&mut self, pos: SectionPos, section: Arc<Section>, hash: u64) {
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
            // O(cap) scan of a u64 per over-cap insert — a few µs against
            // unload cadence; not worth an ordered side structure.
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

    /// Take the cached copy for a `SectionCached { pos, hash }` re-promotion.
    /// `None` = miss (never parked, cap-evicted, or a hash that disagrees
    /// with the server's belief) — the caller answers `SectionCacheMiss` and
    /// the server re-streams the full payload. A disagreeing entry is dropped
    /// either way: it is provably not what the server vouches for.
    pub(crate) fn promote(&mut self, pos: SectionPos, hash: u64) -> Option<Arc<Section>> {
        let entry = self.entries.remove(&pos)?;
        (entry.hash == hash).then_some(entry.section)
    }

    /// Drop a parked copy the server superseded with a full `SectionData`
    /// (its content moved while the section was unloaded).
    pub(crate) fn discard(&mut self, pos: SectionPos) {
        self.entries.remove(&pos);
    }

    /// The Join-manifest claims, oldest-first so the server's belief map
    /// seeds in this cache's insertion order and both caps keep evicting the
    /// same entries.
    pub(crate) fn claims(&self) -> Vec<SectionCacheClaim> {
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

    /// Bind the cache to a session's id vocabulary, clearing it when the
    /// vocabulary moved: cached blocks are client-local ids, and a session
    /// whose server tables read differently would re-promote them as the
    /// wrong blocks even where the server-domain hash still matches.
    pub(crate) fn adopt_session(&mut self, registry_key: u64) {
        if self.registry_key != Some(registry_key) {
            self.entries.clear();
        }
        self.registry_key = Some(registry_key);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, pos: SectionPos) -> bool {
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
