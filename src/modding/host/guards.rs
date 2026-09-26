//! Guards and lookups shared by every call handler: namespace/size write
//! guards, the sim-scope gate, and registry validation helpers.

use mod_api::{HostCall, HostRet};

use crate::events::SimCtx;
use crate::modding::scope;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::Block;
use petramond_world::item::ItemType;

/// Every bound this module enforces is declared in the ABI crate, because a
/// mod has to obey them and can only do that by reading them — the SDK
/// re-exports these same items. Violations are [`HostRet::Error`]: a mod bug,
/// surfaced loudly by the SDK (panic → mod disabled).
///
/// Why these particular numbers. The watchdog deliberately charges GUEST
/// compute only, so host-side per-element work is unmetered: without
/// [`SIM_BATCH_MAX`] one maximal batch (the 64 MiB guest memory allows
/// millions of positions) stalls the sim with no backstop, and 4096 is orders
/// of magnitude above legitimate per-tick batches (bundled mods peak in the
/// low hundreds) while a maximal capped batch stays microseconds of host work.
/// It mirrors the client surface's per-call caps (`CLIENT_BLOCKS_QUERY_MAX`
/// etc.). [`CELL_KV_MAX_KEYS`] is an order of magnitude above real per-cell
/// state (the dye pot peaks at 2) while keeping a maximal cell's `BlockDelta` a
/// bounded wire payload.
pub(in crate::modding) use mod_api::KV_MAX_KEY_BYTES;
pub(super) use mod_api::{
    CELL_KV_MAX_KEYS, EVENT_MAX_DATA_BYTES, FIND_BLOCKS_VOLUME_MAX, KV_MAX_VALUE_BYTES,
    SIM_BATCH_MAX,
};

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
    let engine_owned = key_owned_by_namespace(petramond_world::registry::ENGINE_NAMESPACE, key);
    if !(mod_owned || engine_owned) {
        return Some(HostRet::Error(format!(
            "mod writes must use this mod's own namespace ('{mod_id}:name') or an engine-owned \
             '{engine}:name' key; got '{key}' (reads may cross namespaces)",
            engine = petramond_world::registry::ENGINE_NAMESPACE
        )));
    }
    None
}

/// The actor a single-player-era call addresses implicitly, or the refusal
/// an actor-less dispatch answers it with: there is no privileged player to
/// fall back on. `twin` names the explicit call that does the same thing.
pub(super) fn actor_for(
    ctx: &SimCtx<'_>,
    call: &str,
    twin: &str,
) -> Result<crate::player::PlayerId, HostRet> {
    ctx.actor.ok_or_else(|| {
        HostRet::Error(format!(
            "{call} addresses the acting player, and this dispatch (a tick system, block hook, \
             spawn pick, mod_init or mob action) has none; name the player with {twin}"
        ))
    })
}

/// The reply every exclusive-access wrapper gives inside a read-only
/// dispatch.
fn read_only_refusal() -> HostRet {
    HostRet::Error(
        "this host call needs write access to the world, which a read-only dispatch \
         (e.g. a shape placement plan) does not grant"
            .into(),
    )
}

/// The reply when no guest dispatch published a simulation context.
fn no_context() -> HostRet {
    HostRet::Error("no simulation context is active".into())
}

/// Whether `call` is legal inside a READ-ONLY dispatch (the shape
/// placement-plan dispatch, whose ABI promises the guest cannot edit the
/// world it validates against). The switchboard refuses everything else
/// before routing, so this is the one place the read-only promise is
/// written down — an allow-list, so a call added to the ABI stays refused
/// until someone decides it is a pure read.
///
/// Behind it, a read-only scope lends the [`SimCtx`] SHARED only
/// ([`scope::with_active_ref`]): every allowed call that reads the world
/// goes through [`sim_read`], and the exclusive wrappers below refuse, so a
/// call listed here by mistake still cannot mutate. When the ABI grows a
/// per-call legality table, this function becomes a lookup into it.
pub(in crate::modding) fn read_only_permits(call: &HostCall) -> bool {
    matches!(
        call,
        // Store-local and scope-free.
        HostCall::Log { .. }
            | HostCall::RuntimeSide
            | HostCall::CurrentTick
            | HostCall::RngU64 { .. }
            // World reads (through `sim_read`).
            | HostCall::GetBlock { .. }
            | HostCall::GetBlocks { .. }
            | HostCall::BlockChangesSince { .. }
            | HostCall::IsLoaded { .. }
            | HostCall::LightAt { .. }
            | HostCall::CollisionShapeAt { .. }
            | HostCall::BiomeAt { .. }
            | HostCall::SurfaceYAt { .. }
            | HostCall::FindBlocks { .. }
            | HostCall::BlockLocalToWorld { .. }
            | HostCall::Raycast { .. }
            | HostCall::ItemEntitiesInRadius { .. }
            | HostCall::WorldKvGet { .. }
            | HostCall::SectionKvGet { .. }
            | HostCall::SectionKvFind { .. }
            | HostCall::SectionKvGetMany { .. }
            | HostCall::MobTagGet { .. }
            | HostCall::MobTagsGet { .. }
            | HostCall::MobsWithTag { .. }
            | HostCall::BlockRecordsAt { .. }
            | HostCall::BlockRecordStatuses { .. }
            | HostCall::ActingPlayer
            | HostCall::PlayerIdentity { .. }
            // Catalog lookups: pure functions of the loaded registries.
            | HostCall::BlockRecordPlans { .. }
            | HostCall::StructureInfo { .. }
            | HostCall::LootRoll { .. }
            | HostCall::MobDataGet { .. }
            | HostCall::MobsWithData { .. }
            | HostCall::ResolveBlock { .. }
            | HostCall::ResolveItem { .. }
            | HostCall::ResolveMob { .. }
            | HostCall::BlockNames { .. }
            | HostCall::ItemNames { .. }
            | HostCall::MobNames { .. }
            | HostCall::ResolveCondition { .. }
            | HostCall::ConditionNames { .. }
            | HostCall::BlocksByTag { .. }
            | HostCall::ItemsByTag { .. }
            | HostCall::ItemInfo { .. }
            | HostCall::ResolveShape { .. }
            | HostCall::ItemDataGet { .. }
            | HostCall::ItemsWithData { .. }
            | HostCall::BlockDataGet { .. }
            | HostCall::BlocksWithData { .. }
            | HostCall::BlockInfo { .. }
            | HostCall::BlockInfos { .. }
            // Pure positional worldgen queries and memo reads.
            | HostCall::ResolveUndergroundBiome { .. }
            | HostCall::UndergroundBiomeAt { .. }
            | HostCall::UndergroundBiomesInBox { .. }
            | HostCall::TerrainBlocksAt { .. }
            | HostCall::TerrainSectionAt { .. }
            | HostCall::TerrainHeightsAt { .. }
            | HostCall::TerrainSolidAt { .. }
            | HostCall::TerrainSpaceAt { .. }
            | HostCall::SurfaceBiomeAt { .. }
            | HostCall::MemoGet { .. }
            | HostCall::MemoGetMany { .. }
    )
}

/// Run a call that mutates the live simulation, or reject it when no guest
/// dispatch scope is active (the same gate `CurrentTick` uses), or when the
/// active dispatch is READ-ONLY (which lends no exclusive access at all).
pub(super) fn sim_call(f: impl FnOnce(&mut SimCtx<'_>)) -> HostRet {
    sim_query(|ctx| {
        f(ctx);
        HostRet::Unit
    })
}

/// [`sim_call`] for a mutation that first RESOLVES something off the live
/// context and may refuse (an attacker to validate, an owner to check): the
/// closure's `Err` is the reply, `Ok` is [`HostRet::Unit`].
pub(super) fn sim_mutate(f: impl FnOnce(&mut SimCtx<'_>) -> Result<(), HostRet>) -> HostRet {
    sim_query(|ctx| f(ctx).err().unwrap_or(HostRet::Unit))
}

/// [`sim_call`] for calls that compute their own reply (a spawn answering an
/// id, a spend answering the taken stack, a read that has to borrow a player
/// through the roster's lending API): EXCLUSIVE access to the live context,
/// so it is refused in a read-only dispatch like every other writer. A call
/// that only reads the world uses [`sim_read`] instead.
pub(super) fn sim_query(f: impl FnOnce(&mut SimCtx<'_>) -> HostRet) -> HostRet {
    if scope::read_only_active() {
        return read_only_refusal();
    }
    scope::with_active(f).unwrap_or_else(no_context)
}

/// A world READ: SHARED access to the live context, so the closure cannot
/// mutate it (that fails to compile). The only wrapper a read-only dispatch
/// answers.
pub(super) fn sim_read(f: impl FnOnce(&SimCtx<'_>) -> HostRet) -> HostRet {
    scope::with_active_ref(f).unwrap_or_else(no_context)
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

/// Stream-final gate for WRITE-through-a-cell arms (`SwapBlock`,
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

/// A mod-supplied world position, rejected unless every component is finite.
pub(super) fn finite_pos(
    v: [f64; 3],
    what: &str,
) -> Result<petramond_math::world_pos::WorldPos, HostRet> {
    if v.iter().all(|c| c.is_finite()) {
        // Every mod world point lands inside the world border.
        Ok(petramond_world::border::clamp(
            petramond_math::world_pos::WorldPos::from_array(v),
        ))
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
    petramond_world::registry::names()
        .items
        .name(item.id())
        .unwrap_or("?")
}

/// An engine stack as its ABI crossing (registry name + count + data).
pub(in crate::modding) fn item_stack_data(
    stack: petramond_world::item::ItemStack,
) -> mod_api::ItemStackData {
    let data = petramond_world::item::variant::get(stack.variant)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    mod_api::ItemStackData {
        item: item_name(stack.item).to_owned(),
        count: stack.count,
        data,
    }
}

/// An ABI instance-data list as an interned [`petramond_world::item::VariantId`].
/// Empty = `NONE`. A duplicate key, bare key, or over-cap map is a HARD error
/// (`Err(HostRet::Error)` — loud mod bug, same shape as the KV size caps):
/// silently degrading a write the mod asked for would fork its view of the
/// stack from the engine's. A FULL variant table is NOT a mod bug — the map
/// is well-formed, the process just ran out of ids — so it degrades to a
/// plain stack with a warning instead of disabling the mod.
pub(super) fn intern_abi_data(
    what: &str,
    data: &[(String, Vec<u8>)],
) -> Result<petramond_world::item::VariantId, mod_api::HostRet> {
    use petramond_world::item::variant;
    if data.is_empty() {
        return Ok(variant::VariantId::NONE);
    }
    let map = abi_data_map(what, data)?;
    Ok(variant::intern(&map).unwrap_or_else(|e| {
        log::warn!("{what}: {e} — stack degrades to plain");
        variant::VariantId::NONE
    }))
}

/// An ABI instance-data list as a validated [`VariantMap`] — the COMPARE
/// half of the data surface: a map a call matches against what is carried
/// must never be interned (the table never evicts), so it stops here.
///
/// [`VariantMap`]: petramond_world::item::variant::VariantMap
pub(super) fn abi_data_map(
    what: &str,
    data: &[(String, Vec<u8>)],
) -> Result<petramond_world::item::variant::VariantMap, mod_api::HostRet> {
    use petramond_world::item::variant;
    let mut map = variant::VariantMap::new();
    for (k, v) in data {
        if map.insert(k.clone(), v.clone()).is_some() {
            return Err(mod_api::HostRet::Error(format!(
                "{what}: duplicate instance-data key '{k}'"
            )));
        }
    }
    if !map.is_empty() && !variant::valid(&map) {
        return Err(mod_api::HostRet::Error(format!(
            "{what}: invalid instance data (bare key or over-cap map/value)"
        )));
    }
    Ok(map)
}

#[cfg(test)]
mod tests;
