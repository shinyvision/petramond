//! Persistent mod KV: world-level (rides `level.dat`), per-section-cell
//! (rides the section record), and per-mob (rides the mob record).
//!
//!
//! Keys are namespaced. Writes must use this mod's own prefix or an exposed
//! engine `petramond:*` key (enforced by the host; a violation panics = disables the
//! mod); reads may cross namespaces — the cross-mod interop surface. Key ≤ 256
//! bytes, value ≤ 64 KiB.

use mod_api::{HostCall, HostRet};

use crate::__rt;

/// Read a world KV entry (persists in the save's `level.dat`).
pub fn world_kv_get(key: &str) -> Option<Vec<u8>> {
    match __rt::host_call(&HostCall::WorldKvGet { key: key.into() }) {
        HostRet::Bytes(v) => v,
        other => panic!("WorldKvGet returned {other:?}"),
    }
}

/// Write a world KV entry (own namespace or exposed `petramond:*` key required).
pub fn world_kv_set(key: &str, value: Vec<u8>) {
    __rt::expect_unit(
        "WorldKvSet",
        __rt::host_call(&HostCall::WorldKvSet {
            key: key.into(),
            value,
        }),
    );
}

/// Delete a world KV entry (own namespace or exposed `petramond:*` key required);
/// `false` = absent.
pub fn world_kv_delete(key: &str) -> bool {
    match __rt::host_call(&HostCall::WorldKvDelete { key: key.into() }) {
        HostRet::Bool(present) => present,
        other => panic!("WorldKvDelete returned {other:?}"),
    }
}

/// Read a per-cell KV entry (`pos` = world block position). `None` when the
/// key is absent or the owning section is unloaded.
pub fn section_kv_get(pos: [i32; 3], key: &str) -> Option<Vec<u8>> {
    match __rt::host_call(&HostCall::SectionKvGet {
        pos,
        key: key.into(),
    }) {
        HostRet::Bytes(v) => v,
        other => panic!("SectionKvGet returned {other:?}"),
    }
}

/// Write a per-cell KV entry (own-namespace key required). `false` = the
/// owning section is unloaded (nothing stored). Cell KV is per-BLOCK state:
/// it dies with the block when the cell is broken/replaced (a
/// `swap_model_block` flip carries it across) — never rely on it outliving
/// your placed block.
pub fn section_kv_set(pos: [i32; 3], key: &str, value: Vec<u8>) -> bool {
    match __rt::host_call(&HostCall::SectionKvSet {
        pos,
        key: key.into(),
        value,
    }) {
        HostRet::Bool(stored) => stored,
        other => panic!("SectionKvSet returned {other:?}"),
    }
}

/// Delete a per-cell KV entry (own-namespace key required); `false` = absent.
pub fn section_kv_delete(pos: [i32; 3], key: &str) -> bool {
    match __rt::host_call(&HostCall::SectionKvDelete {
        pos,
        key: key.into(),
    }) {
        HostRet::Bool(present) => present,
        other => panic!("SectionKvDelete returned {other:?}"),
    }
}

/// Read a per-mob KV entry (`mob_index` valid this tick only). `None` when
/// the key is absent or there is no such mob.
pub fn mob_kv_get(mob_index: u32, key: &str) -> Option<Vec<u8>> {
    match __rt::host_call(&HostCall::MobKvGet {
        mob_index,
        key: key.into(),
    }) {
        HostRet::Bytes(v) => v,
        other => panic!("MobKvGet returned {other:?}"),
    }
}

/// Write a per-mob KV entry (own-namespace key required); persists with the
/// mob's save record. `false` = no such mob.
pub fn mob_kv_set(mob_index: u32, key: &str, value: Vec<u8>) -> bool {
    match __rt::host_call(&HostCall::MobKvSet {
        mob_index,
        key: key.into(),
        value,
    }) {
        HostRet::Bool(stored) => stored,
        other => panic!("MobKvSet returned {other:?}"),
    }
}

/// Delete a per-mob KV entry (own-namespace key required); `false` = absent.
pub fn mob_kv_delete(mob_index: u32, key: &str) -> bool {
    match __rt::host_call(&HostCall::MobKvDelete {
        mob_index,
        key: key.into(),
    }) {
        HostRet::Bool(present) => present,
        other => panic!("MobKvDelete returned {other:?}"),
    }
}
