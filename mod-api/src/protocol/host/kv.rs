//! Persistent mod key/value state: world-level and per section cell.
//!
//! Keys are namespaced. WRITES (set/delete) must use the calling mod's own
//! prefix or an exposed engine `petramond:*` key; READS may cross namespaces —
//! that is the cross-mod interop surface. Bounds: [`KV_MAX_KEY_BYTES`],
//! [`KV_MAX_VALUE_BYTES`] and [`CELL_KV_MAX_KEYS`]; exceeding one is refused
//! with [`ErrorCode::LimitExceeded`], a namespace violation with
//! [`ErrorCode::Forbidden`].
//!
//! [`KV_MAX_KEY_BYTES`]: crate::KV_MAX_KEY_BYTES
//! [`KV_MAX_VALUE_BYTES`]: crate::KV_MAX_VALUE_BYTES
//! [`CELL_KV_MAX_KEYS`]: crate::CELL_KV_MAX_KEYS
//! [`ErrorCode::LimitExceeded`]: crate::ErrorCode::LimitExceeded
//! [`ErrorCode::Forbidden`]: crate::ErrorCode::Forbidden
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::legality::prelude::*;

host_domain! {
    /// Persistent mod key/value state: world-level and per section cell.
    KvCall {
        /// World KV (persists in `level.dat`). → [`HostRet::Bytes`](crate::HostRet::Bytes).
        WorldKvGet {
            key: String,
        } => legal(SERVER, Sim, Read),
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        WorldKvSet {
            key: String,
            value: Vec<u8>,
        } => legal(SERVER, Sim, Write),
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (whether the key was present).
        WorldKvDelete {
            key: String,
        } => legal(SERVER, Sim, Write),
        /// Per-cell KV riding the cell's section save record (`pos` is a world
        /// block position). `Bytes(None)` when absent OR the section is unloaded.
        /// → [`HostRet::Bytes`](crate::HostRet::Bytes).
        SectionKvGet {
            pos: [i32; 3],
            key: String,
        } => legal(SERVER, Sim, Read),
        /// `false` = the section is unloaded (nothing stored). Cell KV is
        /// per-BLOCK state: breaking/replacing the cell's block clears it (a
        /// `SwapBlock` flip carries it across). → [`HostRet::Bool`](crate::HostRet::Bool).
        SectionKvSet {
            pos: [i32; 3],
            key: String,
            value: Vec<u8>,
        } => legal(SERVER, Sim, Write),
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (whether the key was present).
        SectionKvDelete {
            pos: [i32; 3],
            key: String,
        } => legal(SERVER, Sim, Write),
        /// [`SectionKvGet`](Self::SectionKvGet) for ONE key across many cells —
        /// the shape a machine kind reads its own blob in (every placed machine of
        /// a kind stores its state under the same key). Reply parallel to
        /// `positions` ([`HostRet::BytesMany`](crate::HostRet::BytesMany)); `None` = absent or unloaded, the
        /// same answers as the single call. Bounded batch
        /// ([`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX)).
        SectionKvGetMany {
            key: String,
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        /// [`SectionKvSet`](Self::SectionKvSet) / [`SectionKvDelete`] for one key
        /// across many cells: `None` value = delete. Reply parallel to `writes`
        /// ([`HostRet::Bools`](crate::HostRet::Bools)) — `false` = the owning section is unloaded
        /// (nothing stored), a delete of an absent key, or a NEW key on a cell
        /// already holding [`CELL_KV_MAX_KEYS`](crate::CELL_KV_MAX_KEYS). That
        /// last one is an ERROR on the single call: a batch is a whole machine
        /// kind's writes, and one cell over the cap must not take the pack down.
        /// Own-namespace key required, like the single call. Bounded batch
        /// ([`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX)).
        ///
        /// [`SectionKvDelete`]: Self::SectionKvDelete
        SectionKvSetMany {
            key: String,
            writes: Vec<([i32; 3], Option<Vec<u8>>)>,
        } => legal(SERVER, Sim, Write),
        /// Cells carrying `key` in one 16³ section, sorted by local cell index.
        /// `None` while the section is unloaded or awaiting saved terrain; retry
        /// later. Reads only the sparse data map. → [`HostRet::FoundBlocks`](crate::HostRet::FoundBlocks).
        SectionKvFind {
            section: [i32; 3],
            key: String,
        } => legal(SERVER, Sim, Read),
    }
}
