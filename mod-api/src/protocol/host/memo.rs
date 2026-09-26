//! The shared derived-fact memo, scoped to (mod, world seed): settled
//! positional facts one instance derives and every instance of the mod reuses.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::legality::prelude::*;

host_domain! {
    /// The shared derived-fact memo, scoped to (mod, world seed): settled
    /// positional facts one instance derives and every instance of the mod reuses.
    MemoCall {
        /// Read one entry of the shared derived-fact memo (see [`Self::MemoPut`]).
        /// `Bytes(None)` = never stored, or evicted since. Legal on every instance.
        MemoGet {
            #[serde(with = "serde_bytes")]
            key: Vec<u8>,
        } => legal(EVERY, Any, Read),
        /// [`Self::MemoGet`] over many keys, reply parallel to `keys`
        /// ([`HostRet::BytesMany`](crate::HostRet::BytesMany)). At most `SIM_BATCH_MAX` keys.
        MemoGetMany {
            keys: Vec<Vec<u8>>,
        } => legal(EVERY, Any, Read),
        /// Publish an entry to the memo every instance of the calling mod shares —
        /// all threads, all runtime sides — scoped to the mod and the world seed.
        /// Bounded and discardable: an entry may vanish at any time, so a value
        /// must be a pure function of the world seed and its key (a settled
        /// positional decision), never state. `Bool(false)` = the value exceeds
        /// `MEMO_MAX_VALUE_BYTES` and was not stored; a key over
        /// `MEMO_MAX_KEY_BYTES` errors.
        MemoPut {
            #[serde(with = "serde_bytes")]
            key: Vec<u8>,
            #[serde(with = "serde_bytes")]
            value: Vec<u8>,
        } => legal(EVERY, Any, Write),
        /// [`Self::MemoGet`] that also settles WHO derives a missing entry: the
        /// first caller to miss holds the lease and must [`Self::MemoPut`] the
        /// value; a caller missing while a lease is held waits briefly for that
        /// value, and past that wait is told the derivation is pending, so a
        /// generation callback can defer its section instead of idling or
        /// deriving the same fact on every worker. → [`HostRet::MemoClaim`](crate::HostRet::MemoClaim).
        MemoClaim {
            #[serde(with = "serde_bytes")]
            key: Vec<u8>,
        } => legal(EVERY, Any, Write),
    }
}
