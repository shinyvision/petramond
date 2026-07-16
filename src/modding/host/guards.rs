//! Guards and lookups shared by every call handler: namespace/size write
//! guards, the sim-scope gate, and registry validation helpers.

use mod_api::HostRet;

use crate::block::Block;
use crate::events::SimCtx;
use crate::item::ItemType;
use crate::mathh::Vec3;
use crate::modding::scope;

/// Per-entry limits for the mod KV surfaces (world / section-cell / mob).
/// Violations are [`HostRet::Error`] — a mod bug, surfaced loudly by the SDK.
pub(super) const KV_MAX_KEY_BYTES: usize = 256;
pub(super) const KV_MAX_VALUE_BYTES: usize = 64 * 1024;

/// The mod-KV write guard: WRITES (set/delete) must use either the calling
/// mod's own `mod_id:` prefix or an exposed engine `petramond:` key. Reads may cross
/// namespaces (the interop surface), and keys/values are size-capped.
/// `Some(err)` rejects the call.
pub(super) fn kv_write_guard(mod_id: &str, key: &str, value_len: usize) -> Option<HostRet> {
    if key.len() > KV_MAX_KEY_BYTES {
        return Some(HostRet::Error(format!(
            "KV key is {} bytes; the limit is {KV_MAX_KEY_BYTES}",
            key.len()
        )));
    }
    if value_len > KV_MAX_VALUE_BYTES {
        return Some(HostRet::Error(format!(
            "KV value is {value_len} bytes; the limit is {KV_MAX_VALUE_BYTES}"
        )));
    }
    public_write_key_guard(mod_id, key)
}

pub(in crate::modding) fn key_owned_by_namespace(namespace: &str, key: &str) -> bool {
    key.strip_prefix(namespace)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|name| !name.is_empty())
}

pub(super) fn public_write_key_guard(mod_id: &str, key: &str) -> Option<HostRet> {
    let mod_owned = key_owned_by_namespace(mod_id, key);
    let engine_owned = key_owned_by_namespace(crate::registry::ENGINE_NAMESPACE, key);
    if !(mod_owned || engine_owned) {
        return Some(HostRet::Error(format!(
            "mod writes must use this mod's own namespace ('{mod_id}:name') or an engine-owned \
             '{engine}:name' key; got '{key}' (reads may cross namespaces)",
            engine = crate::registry::ENGINE_NAMESPACE
        )));
    }
    None
}

/// Run a call that mutates the live simulation, or reject it when no guest
/// dispatch scope is active (the same gate `CurrentTick` uses).
pub(super) fn sim_call(f: impl FnOnce(&mut SimCtx<'_>)) -> HostRet {
    match scope::with_active(f) {
        Some(()) => HostRet::Unit,
        None => HostRet::Error("no simulation context is active".into()),
    }
}

/// [`sim_call`] for calls that compute their own reply.
pub(super) fn sim_query(f: impl FnOnce(&mut SimCtx<'_>) -> HostRet) -> HostRet {
    scope::with_active(f)
        .unwrap_or_else(|| HostRet::Error("no simulation context is active".into()))
}

/// Validate an ABI block id against the loaded registry — an unregistered id
/// must never reach world storage.
pub(super) fn checked_block(block: mod_api::BlockId) -> Result<Block, HostRet> {
    if (block.0 as usize) < Block::all().len() {
        Ok(Block(block.0))
    } else {
        Err(HostRet::Error(format!(
            "unregistered block id {} (ids are session-scoped; resolve them from your own \
             catalog rows, never persist them)",
            block.0
        )))
    }
}

/// Reject non-finite guest floats before they reach engine state (NaNs are
/// canonicalized by wasmtime but still NaN; infinities pass through).
pub(super) fn finite3(v: [f32; 3], what: &str) -> Result<Vec3, HostRet> {
    if v.iter().all(|c| c.is_finite()) {
        Ok(v.into())
    } else {
        Err(HostRet::Error(format!("{what}: non-finite component")))
    }
}

/// The runtime item registered under `key` (`ItemType::key` — the stable
/// snake_case identity, `mod_id:name` for pack items).
pub(super) fn item_by_key(key: &str) -> Option<ItemType> {
    ItemType::all().iter().copied().find(|i| i.key() == key)
}

/// An engine stack as its ABI crossing (registry key + count).
pub(super) fn item_stack_data(stack: crate::item::ItemStack) -> mod_api::ItemStackData {
    mod_api::ItemStackData {
        key: stack.item.key().to_owned(),
        count: stack.count,
    }
}
