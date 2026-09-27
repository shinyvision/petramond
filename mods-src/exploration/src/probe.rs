//! Cross-section positional queries: the one seam every worldgen pass in this
//! pack reads the world beyond its own section through.
//!
//! SEAM CONTRACT. Sections generate independently, in any order, on any
//! thread, and the engine clips each dispatch's writes to its own section. A
//! structure that spans sections is therefore re-derived by every section it
//! reaches, and each of them must arrive at the identical answer: every
//! decision is a pure function of `(world seed, salt, anchor coordinates)`
//! and of reads that answer the same from every section.
//!
//! WHICH READS MAY DECIDE. `GenCtx::block` answers only inside the dispatching
//! section, so it can never carry a decision about a structure that spans
//! sections — the owner would reject what its neighbours accept, and the
//! neighbours would still write their share. Anything that decides WHETHER a
//! multi-section structure exists reads [`TerrainReads`], which is positional.
//! A single-cell decoration needs no agreement, only the truth about its own
//! neighbours: it reads the snapshot for cells it owns and asks
//! [`TerrainReads::ask_unseen`] for the rest, never assuming an unseen cell is
//! air.
//!
//! HOST-CALL BUDGET. Every query here is an ABI crossing, not a field sampler.
//! Candidates are filtered by their free positional rolls first, then asked
//! about in one batch per question ([`ask`], split at the ABI cap rather than
//! truncated), and a short reply is never read as an answer. [`in_reach`] is
//! the bounded gate a pass pays before gathering anything at all.
//!
//! MEMOIZATION. A fact that costs real crossings (a site probe, a containment
//! proof, a root verdict) is published to the shared memo so the first worker
//! to derive it serves the rest ([`settle`], [`lookup_many`]), behind a
//! per-worker [`Settled`] cache. The memo never bends the seam contract: its
//! values are pure functions of the seed and the key, so visit order changes
//! who computes a fact first, never what it is.

use std::collections::VecDeque;
use std::hash::Hash;

use mod_sdk::*;

pub(crate) type Query<T> = fn(Vec<[i32; 3]>) -> Vec<T>;

pub(crate) fn ask<T>(positions: Vec<[i32; 3]>, call: Query<T>) -> Option<Vec<T>> {
    let want = positions.len();
    let reply = paged(positions, call);
    (reply.len() == want).then_some(reply)
}

pub(crate) struct TerrainReads {
    query: Query<TerrainSpace>,
    answers: FxHashMap<[i32; 3], TerrainSpace>,
    failed: bool,
}

impl TerrainReads {
    pub(crate) fn new() -> TerrainReads {
        TerrainReads::with_query(terrain_space_at)
    }

    pub(crate) fn with_query(query: Query<TerrainSpace>) -> TerrainReads {
        TerrainReads {
            query,
            answers: FxHashMap::default(),
            failed: false,
        }
    }

    pub(crate) fn ask(&mut self, cells: impl IntoIterator<Item = [i32; 3]>) -> bool {
        if self.failed {
            return false;
        }
        let mut seen = FxHashSet::default();
        let fresh: Vec<[i32; 3]> = cells
            .into_iter()
            .filter(|c| !self.answers.contains_key(c) && seen.insert(*c))
            .collect();
        if fresh.is_empty() {
            return true;
        }
        match ask(fresh.clone(), self.query) {
            Some(reply) => {
                self.answers.extend(fresh.into_iter().zip(reply));
                true
            }
            None => {
                self.answers.clear();
                self.failed = true;
                false
            }
        }
    }

    pub(crate) fn ask_unseen(
        &mut self,
        ctx: &GenCtx,
        cells: impl IntoIterator<Item = [i32; 3]>,
    ) -> bool {
        self.ask(cells.into_iter().filter(|&c| ctx.block(c).is_none()))
    }

    pub(crate) fn space(&self, c: [i32; 3]) -> Option<TerrainSpace> {
        self.answers.get(&c).copied()
    }

    #[cfg(test)]
    pub(crate) fn solid(&self, c: [i32; 3]) -> Option<bool> {
        self.space(c).map(|s| s == TerrainSpace::Solid)
    }

    pub(crate) fn rock(&self, c: [i32; 3]) -> bool {
        self.space(c) == Some(TerrainSpace::Solid)
    }

    pub(crate) fn free(&self, c: [i32; 3]) -> bool {
        self.space(c) == Some(TerrainSpace::Air)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Pad {
    pub(crate) xz: i32,
    pub(crate) down: i32,
    pub(crate) up: i32,
}

impl Pad {
    pub(crate) fn around(self, origin: [i32; 3]) -> ([i32; 3], [i32; 3]) {
        (
            [
                origin[0] - self.xz,
                origin[1] - self.down,
                origin[2] - self.xz,
            ],
            [
                origin[0] + 15 + self.xz,
                origin[1] + 15 + self.up,
                origin[2] + 15 + self.xz,
            ],
        )
    }
}

pub(crate) fn in_reach(
    ours: u8,
    origin: [i32; 3],
    pad: Pad,
    biomes_in_box: fn([i32; 3], [i32; 3]) -> Vec<u8>,
) -> bool {
    let (lo, hi) = pad.around(origin);
    biomes_in_box(lo, hi).contains(&ours)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Deferred;

#[derive(Copy, Clone)]
pub(crate) struct Memo {
    pub(crate) claim: fn(&[u8]) -> MemoClaim,
    pub(crate) put: fn(&[u8], Vec<u8>) -> bool,
}

impl Memo {
    pub(crate) const HOST: Memo = Memo {
        claim: memo_blob_claim,
        put: publish_logged,
    };
}

fn publish_logged(key: &[u8], value: Vec<u8>) -> bool {
    let len = value.len();
    let stored = memo_blob_put(key, value);
    if !stored {
        log(&format!(
            "memo refused a {len}-byte fact (limit {MEMO_BLOB_MAX_BYTES}); \
             other workers re-derive it"
        ));
    }
    stored
}

pub(crate) fn settle<V>(
    memo: Memo,
    key: &[u8],
    decode: impl FnOnce(&[u8]) -> Option<V>,
    encode: impl FnOnce(&V) -> Vec<u8>,
    derive: impl FnOnce() -> V,
) -> Result<V, Deferred> {
    let published = match (memo.claim)(key) {
        MemoClaim::Value(bytes) => decode(&bytes),
        MemoClaim::Lease => None,
        MemoClaim::Pending => return Err(Deferred),
    };
    Ok(published.unwrap_or_else(|| {
        let value = derive();
        (memo.put)(key, encode(&value));
        value
    }))
}

type MemoBatchLookup = fn(Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>>;

pub(crate) fn lookup_many(get_many: MemoBatchLookup, keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>> {
    paged(keys, |page| {
        let want = page.len();
        let mut reply = get_many(page);
        reply.resize(want, None);
        reply
    })
}

pub(crate) struct Settled<K, V> {
    entries: FxHashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K: Copy + Eq + Hash, V: Clone> Settled<K, V> {
    pub(crate) fn new(capacity: usize) -> Settled<K, V> {
        Settled {
            entries: FxHashMap::default(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn get(&self, key: &K) -> Option<V> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        if let Some(slot) = self.entries.get_mut(&key) {
            *slot = value;
            return;
        }
        if self.entries.len() == self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
        self.entries.insert(key, value);
        self.order.push_back(key);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests;
