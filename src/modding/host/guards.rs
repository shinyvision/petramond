//! Guards and lookups shared by every call handler: namespace/size write
//! guards, the sim-scope gate, and registry validation helpers.

use mod_api::HostRet;

use crate::block::Block;
use crate::events::SimCtx;
use crate::item::ItemType;
use crate::mathh::{IVec3, Vec3};
use crate::modding::scope;

/// Per-entry limits for the mod KV surfaces (world / section-cell / mob).
/// Violations are [`HostRet::Error`] — a mod bug, surfaced loudly by the SDK.
pub(super) const KV_MAX_KEY_BYTES: usize = 256;
pub(super) const KV_MAX_VALUE_BYTES: usize = 64 * 1024;

/// Element cap for every batched sim/registry call (`GetBlocks`, `SetBlocks`,
/// `ContainerGetMany`, `ContainerSet` slots, the `*Names` reverse resolvers,
/// `ChatSend` targets) — the documented ABI bound, mirroring the client
/// surface's per-call caps (`CLIENT_BLOCKS_QUERY_MAX` etc.). The watchdog
/// deliberately charges GUEST compute only, so host-side per-element work is
/// unmetered; without this bound one maximal batch (the 64 MiB guest memory
/// allows millions of positions) stalls the sim with no backstop. 4096 is
/// orders of magnitude above legitimate per-tick batches (bundled mods peak
/// in the low hundreds) while a maximal capped batch stays microseconds of
/// host work. Violations are [`HostRet::Error`] — a mod bug, surfaced loudly
/// by the SDK (panic → mod disabled), like every other cap on this surface.
pub(super) const SIM_BATCH_MAX: usize = 4096;

/// Cell cap for the `FindBlocks` box scan — the same "bounded host work"
/// doctrine as [`SIM_BATCH_MAX`], but a VOLUME bound: the scan pays per cell,
/// not per element. 32³ comfortably covers radius-8..15 neighbourhood
/// searches (17³ = 4913) while a maximal capped scan stays microseconds.
pub(super) const FIND_BLOCKS_VOLUME_MAX: i64 = 32 * 32 * 32;

/// `Some(err)` when a batched call's element count exceeds
/// [`SIM_BATCH_MAX`]; `what` names the call and lane for the error line.
pub(super) fn batch_guard(what: &str, len: usize) -> Option<HostRet> {
    (len > SIM_BATCH_MAX)
        .then(|| HostRet::Error(format!("{what} count {len} exceeds {SIM_BATCH_MAX}")))
}

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
/// dispatch scope is active (the same gate `CurrentTick` uses), or when the
/// active dispatch is READ-ONLY (the shape placement-plan dispatch, whose ABI
/// promises the guest cannot edit the world it validates against).
pub(super) fn sim_call(f: impl FnOnce(&mut SimCtx<'_>)) -> HostRet {
    if scope::read_only_active() {
        return HostRet::Error(
            "this host call mutates the world, which is not allowed during a read-only dispatch \
             (e.g. a shape placement plan)"
                .into(),
        );
    }
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

/// Resolve a stable mob id to its live-list index — the ONE dead-mob policy
/// for every id-addressed mob call arm: a dead (ragdolling) mob is GONE to
/// the ABI, exactly as `MobsInRadius` never lists it, so `None` covers
/// missing and dead alike. Readers then answer `None`/`false`, writers
/// refuse — a corpse is neither readable nor writable. (`MobMount` reaches
/// the same rule through `World::try_mount_player`, its engine seam;
/// `DamageMob` re-resolves at its action drain, where the pipeline rejects
/// the dead.) The returned index is valid only within the current handler.
pub(super) fn live_mob(ctx: &SimCtx<'_>, mob_id: u64) -> Option<usize> {
    let index = ctx.world.mobs().index_of_id(mob_id)?;
    (!ctx.world.mobs().instances()[index].is_dead()).then_some(index)
}

/// Stream-final gate for WRITE-through-a-cell arms (`SwapModelBlock`,
/// `ContainerSet`): the cell's block, or `Err(Bool(false))` while its section
/// is unloaded or its streamed content is not yet final. During that window a
/// plain read LIES — the generated base shows where the player's saved
/// overlay is about to land — so an ownership check would see a FOREIGN block
/// and misfire as a mod-disabling namespace `Error`. The gated miss is benign
/// (`false` = "not stored, retry later"), exactly like every gated read.
pub(super) fn stream_final_cell(ctx: &SimCtx<'_>, pos: IVec3) -> Result<Block, HostRet> {
    ctx.world
        .block_if_stream_final(pos.x, pos.y, pos.z)
        .ok_or(HostRet::Bool(false))
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

/// The runtime item registered under registry NAME `name` — the one
/// mod-facing item identity. O(1) through the shared name index.
pub(super) fn item_by_name(name: &str) -> Option<ItemType> {
    ItemType::by_name(name)
}

/// An item's registry NAME (every registered item has one; `"?"` guards the
/// unreachable unregistered case).
pub(super) fn item_name(item: ItemType) -> &'static str {
    crate::registry::names()
        .items
        .name(item.id())
        .unwrap_or("?")
}

/// An engine stack as its ABI crossing (registry name + count).
pub(super) fn item_stack_data(stack: crate::item::ItemStack) -> mod_api::ItemStackData {
    mod_api::ItemStackData {
        item: item_name(stack.item).to_owned(),
        count: stack.count,
    }
}
