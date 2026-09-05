//! Persistent mod KV: world-level (rides `level.dat`) and per-section-cell
//! (rides the section record). Per-mob keyed data rides the typed TAG map
//! instead — see [`mob_tag_get`](crate::mob_tag_get) and friends.
//!
//! Keys are namespaced. Writes must use this mod's own prefix or an exposed
//! engine `petramond:*` key (enforced by the host; a violation panics = disables the
//! mod); reads may cross namespaces — the cross-mod interop surface. Keys and values
//! are capped at [`crate::KV_MAX_KEY_BYTES`] / [`crate::KV_MAX_VALUE_BYTES`];
//! a list longer than one value SHARDS across keys.

use crate::__rt::host_fn;

host_fn! {
    /// Read a world KV entry (persists in the save's `level.dat`).
    pub fn world_kv_get(key: &str) -> Option<Vec<u8>> => WorldKvGet { key: key.into() } => Bytes
}

host_fn! {
    /// Write a world KV entry (own namespace or exposed `petramond:*` key required).
    pub fn world_kv_set(key: &str, value: Vec<u8>) => WorldKvSet { key: key.into(), value }
}

host_fn! {
    /// Delete a world KV entry (own namespace or exposed `petramond:*` key required);
    /// `false` = absent.
    pub fn world_kv_delete(key: &str) -> bool => WorldKvDelete { key: key.into() } => Bool
}

host_fn! {
    /// Read a per-cell KV entry (`pos` = world block position). `None` when the
    /// key is absent or the owning section is unloaded.
    pub fn section_kv_get(pos: [i32; 3], key: &str) -> Option<Vec<u8>>
        => SectionKvGet { pos, key: key.into() } => Bytes
}

host_fn! {
    /// Write a per-cell KV entry (own-namespace key required). `false` = the
    /// owning section is unloaded (nothing stored). Cell KV is per-BLOCK state:
    /// it dies with the block when the cell is broken/replaced (a
    /// `swap_block` flip carries it across) — never rely on it outliving
    /// your placed block.
    pub fn section_kv_set(pos: [i32; 3], key: &str, value: Vec<u8>) -> bool
        => SectionKvSet { pos, key: key.into(), value } => Bool
}

host_fn! {
    /// Delete a per-cell KV entry (own-namespace key required); `false` = absent.
    pub fn section_kv_delete(pos: [i32; 3], key: &str) -> bool
        => SectionKvDelete { pos, key: key.into() } => Bool
}

host_fn! {
    /// [`section_kv_get`] for ONE key across many cells, reply parallel to
    /// `positions` — the shape a machine kind reads its own state in, since
    /// every placed machine of a kind stores its blob under the same key.
    /// At most [`crate::SIM_BATCH_MAX`] positions.
    pub fn section_kv_get_many(key: &str, positions: Vec<[i32; 3]>) -> Vec<Option<Vec<u8>>>
        => SectionKvGetMany { key: key.into(), positions } => BytesMany
}

host_fn! {
    /// [`section_kv_set`] / [`section_kv_delete`] for one key across many
    /// cells (`None` value = delete), reply parallel to `writes`. At most
    /// [`crate::SIM_BATCH_MAX`] writes.
    pub fn section_kv_set_many(key: &str, writes: Vec<([i32; 3], Option<Vec<u8>>)>) -> Vec<bool>
        => SectionKvSetMany { key: key.into(), writes } => Bools
}
