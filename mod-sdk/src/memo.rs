//! The shared derived-fact memo: a process-wide, bounded, DISCARDABLE store
//! every instance of this mod shares — all worker threads, all runtime sides —
//! scoped by the host to the mod and the world seed.
//!
//! Worldgen instances are per thread and share nothing, so an expensive
//! positional decision (a site probe, a containment proof, a candidate scan)
//! is otherwise re-derived by every worker that touches the cell. Publishing
//! it here lets the first computation serve the rest. An entry may vanish at
//! any time, so a value MUST be a pure function of the world seed and its key:
//! this is a memo, never state — nothing here is saved, replicated or ordered.
//! Keep a local cache in front of it; a crossing per lookup is the cost.

use crate::__rt::host_fn;
use crate::MemoClaim;
mod blob;
pub use blob::{memo_blob_claim, memo_blob_put};

host_fn! {
    /// Read one memo entry; `None` = never stored or since evicted.
    pub fn memo_get(key: &[u8]) -> Option<Vec<u8>> => MemoGet { key: key.to_vec() } => Bytes
}

host_fn! {
    /// [`memo_get`] over many keys, reply parallel to `keys`. At most
    /// [`crate::SIM_BATCH_MAX`] keys.
    pub fn memo_get_many(keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>>
        => MemoGetMany { keys } => BytesMany
}

host_fn! {
    /// [`memo_get`] that also settles who derives a missing entry.
    /// [`MemoClaim::Lease`] means THIS caller must derive and [`memo_put`] the
    /// value; a caller missing while another holds the lease waits briefly,
    /// then gets [`MemoClaim::Pending`] — a generation callback answers that
    /// with [`GenOutput::deferred`](crate::GenOutput::deferred) so its
    /// section runs again once the value exists. Use it for a derivation
    /// worth more than a short wait; a lease never followed by a put expires
    /// after a while and passes to the next claimant.
    pub fn memo_claim(key: &[u8]) -> MemoClaim => MemoClaim { key: key.to_vec() } => MemoClaim
}

host_fn! {
    /// Publish one memo entry. `false` = the value exceeds
    /// [`crate::MEMO_MAX_VALUE_BYTES`] and was not stored (the fact stays
    /// uncached; a key over [`crate::MEMO_MAX_KEY_BYTES`] is a mod bug and errors).
    pub fn memo_put(key: &[u8], value: Vec<u8>) -> bool => MemoPut { key: key.to_vec(), value } => Bool
}
