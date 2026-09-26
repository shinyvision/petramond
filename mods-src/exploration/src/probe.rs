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

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::hash::Hash;

use mod_sdk::*;

/// A positional host query, reply parallel to the request.
pub(crate) type Query<T> = fn(Vec<[i32; 3]>) -> Vec<T>;

/// Run one batched host query, split into ABI-sized pieces.
///
/// Every real section fits in a single crossing — the split exists so a
/// pathological one degrades into a second call instead of having the host
/// REJECT the batch, which would stop the pack generating anything at all, or
/// having a cap TRUNCATE it, which drops candidates by list position and lets a
/// cell that is about to lose to a giant displace a legitimate floor.
pub(crate) fn batched<T>(positions: Vec<[i32; 3]>, call: Query<T>) -> Vec<T> {
    if positions.is_empty() {
        return Vec::new();
    }
    if positions.len() <= SIM_BATCH_MAX {
        return call(positions);
    }
    let mut out = Vec::with_capacity(positions.len());
    for chunk in positions.chunks(SIM_BATCH_MAX) {
        out.extend(call(chunk.to_vec()));
    }
    out
}

/// [`batched`], or `None` when the host answered short. A refused or
/// truncated reply is not a positional answer, so nothing may decide from it.
pub(crate) fn ask<T>(positions: Vec<[i32; 3]>, call: Query<T>) -> Option<Vec<T>> {
    let want = positions.len();
    let reply = batched(positions, call);
    (reply.len() == want).then_some(reply)
}

/// Positional terrain answers, keyed by cell and grown over several asks.
///
/// Answers are looked up by POSITION, never by reply index, so a later ask
/// can never shift an earlier answer onto a different cell. A cell never
/// asked about is UNKNOWN (`None`), and a refused batch makes every cell
/// unknown: a caller that cannot see the world must not decide as if it
/// were open.
pub(crate) struct TerrainReads {
    query: Query<TerrainSpace>,
    answers: HashMap<[i32; 3], TerrainSpace>,
    failed: bool,
}

impl TerrainReads {
    /// Reads answered by the host's `terrain_space_at`.
    pub(crate) fn new() -> TerrainReads {
        TerrainReads::with_query(terrain_space_at)
    }

    /// Reads answered by `query` — the host in play, a synthetic terrain in
    /// tests.
    pub(crate) fn with_query(query: Query<TerrainSpace>) -> TerrainReads {
        TerrainReads {
            query,
            answers: HashMap::new(),
            failed: false,
        }
    }

    /// Ask about every cell not already answered, in one (split) crossing.
    /// Duplicates and answered cells cost nothing. `false` when the host
    /// answered short, after which every read answers `None`.
    pub(crate) fn ask(&mut self, cells: impl IntoIterator<Item = [i32; 3]>) -> bool {
        if self.failed {
            return false;
        }
        let fresh: Vec<[i32; 3]> = cells
            .into_iter()
            .filter(|c| !self.answers.contains_key(c))
            .collect::<BTreeSet<_>>()
            .into_iter()
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

    /// [`ask`](Self::ask) about only the cells outside the dispatching
    /// section: inside it the snapshot is authoritative and a probe would be
    /// a crossing bought for nothing.
    pub(crate) fn ask_unseen(
        &mut self,
        ctx: &GenCtx,
        cells: impl IntoIterator<Item = [i32; 3]>,
    ) -> bool {
        self.ask(cells.into_iter().filter(|&c| ctx.block(c).is_none()))
    }

    /// The answer for `c`, or `None` when it was never asked or the host
    /// refused.
    pub(crate) fn space(&self, c: [i32; 3]) -> Option<TerrainSpace> {
        self.answers.get(&c).copied()
    }

    /// Solid (`Some(true)`), open or fluid (`Some(false)`), or unknown.
    pub(crate) fn solid(&self, c: [i32; 3]) -> Option<bool> {
        self.space(c).map(|s| s == TerrainSpace::Solid)
    }

    /// Known ROCK: what a plant rests on or hangs from.
    pub(crate) fn rock(&self, c: [i32; 3]) -> bool {
        self.space(c) == Some(TerrainSpace::Solid)
    }

    /// Known free ROOM. A fluid cell is neither rock nor room.
    pub(crate) fn free(&self, c: [i32; 3]) -> bool {
        self.space(c) == Some(TerrainSpace::Air)
    }

    /// Whether any answered cell is a fluid.
    pub(crate) fn any_fluid(&self) -> bool {
        self.answers.values().any(|&s| s == TerrainSpace::Fluid)
    }
}

/// How far past the dispatched section a pass can be authorised from:
/// `xz` blocks on every side, `down` below the floor and `up` above the roof.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Pad {
    pub(crate) xz: i32,
    pub(crate) down: i32,
    pub(crate) up: i32,
}

impl Pad {
    /// The inclusive world box this pad spans around the section at `origin`.
    pub(crate) fn around(self, origin: [i32; 3]) -> ([i32; 3], [i32; 3]) {
        (
            [origin[0] - self.xz, origin[1] - self.down, origin[2] - self.xz],
            [
                origin[0] + 15 + self.xz,
                origin[1] + 15 + self.up,
                origin[2] + 15 + self.xz,
            ],
        )
    }
}

/// Can biome `ours` own a cell within `pad` of the section at `origin`? One
/// bounded crossing that does not grow with the candidate count, and
/// conservative in the safe direction: `true` only means "do the real work".
pub(crate) fn in_reach(
    ours: u8,
    origin: [i32; 3],
    pad: Pad,
    biomes_in_box: fn([i32; 3], [i32; 3]) -> Vec<u8>,
) -> bool {
    let (lo, hi) = pad.around(origin);
    biomes_in_box(lo, hi).contains(&ours)
}

/// A section needs a fact another worker is still deriving: the section is
/// dispatched again once it is published.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Deferred;

/// The shared memo's calls, held as values so the settle protocol runs over
/// a fake store in tests.
#[derive(Copy, Clone)]
pub(crate) struct Memo {
    pub(crate) claim: fn(&[u8]) -> MemoClaim,
    pub(crate) put: fn(&[u8], Vec<u8>) -> bool,
    pub(crate) get_many: fn(Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>>,
}

impl Memo {
    /// The host's memo.
    pub(crate) const HOST: Memo = Memo {
        claim: memo_claim,
        put: memo_put,
        get_many: memo_get_many,
    };
}

/// Settle one fact through the memo's lease protocol: a published value is
/// decoded, a lease derives and publishes it, and a lease held elsewhere
/// defers the section. A value that does not decode is re-derived and
/// republished rather than trusted.
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

/// Look many keys up in the memo, split at the ABI cap; the reply is parallel
/// to `keys`, and a key the host did not answer reads as missing.
pub(crate) fn lookup_many(memo: Memo, keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>> {
    let mut out = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(SIM_BATCH_MAX) {
        let mut reply = (memo.get_many)(chunk.to_vec());
        reply.resize(chunk.len(), None);
        out.extend(reply);
    }
    out
}

/// A bounded per-worker cache of settled facts, evicting oldest first.
/// Eviction changes cost, never content: every value is a pure function of
/// its key.
pub(crate) struct Settled<K, V> {
    entries: HashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K: Copy + Eq + Hash, V: Clone> Settled<K, V> {
    pub(crate) fn new(capacity: usize) -> Settled<K, V> {
        Settled {
            entries: HashMap::new(),
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
