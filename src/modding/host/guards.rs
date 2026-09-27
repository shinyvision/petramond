use mod_api::{ErrorCode, HostCall, HostRet};

use crate::events::SimCtx;
use crate::modding::scope;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::Block;
use petramond_world::item::ItemType;

pub(in crate::modding) use mod_api::KV_MAX_KEY_BYTES;
pub(super) use mod_api::{
    CELL_KV_MAX_KEYS, EVENT_MAX_DATA_BYTES, FIND_BLOCKS_VOLUME_MAX, KV_MAX_VALUE_BYTES,
    SIM_BATCH_MAX,
};

pub(super) fn batch_guard(what: &str, len: usize) -> Option<HostRet> {
    (len > SIM_BATCH_MAX).then(|| {
        HostRet::error(
            ErrorCode::LimitExceeded,
            format!("{what} count {len} exceeds {SIM_BATCH_MAX}"),
        )
    })
}

pub(super) fn kv_write_guard(mod_id: &str, key: &str, value_len: usize) -> Option<HostRet> {
    if key.len() > KV_MAX_KEY_BYTES {
        return Some(HostRet::error(
            ErrorCode::LimitExceeded,
            format!(
                "KV key is {} bytes; the limit is {KV_MAX_KEY_BYTES}",
                key.len()
            ),
        ));
    }
    if value_len > KV_MAX_VALUE_BYTES {
        return Some(HostRet::error(
            ErrorCode::LimitExceeded,
            format!("KV value is {value_len} bytes; the limit is {KV_MAX_VALUE_BYTES}"),
        ));
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
        return Some(HostRet::error(
            ErrorCode::Forbidden,
            format!(
                "mod writes must use this mod's own namespace ('{mod_id}:name') or an \
                 engine-owned '{engine}:name' key; got '{key}' (reads may cross namespaces)",
                engine = petramond_world::registry::ENGINE_NAMESPACE
            ),
        ));
    }
    None
}

pub(super) fn actor_for(
    ctx: &SimCtx<'_>,
    call: &str,
    twin: &str,
) -> Result<crate::player::PlayerId, HostRet> {
    ctx.actor.ok_or_else(|| {
        HostRet::error(
            ErrorCode::NoActor,
            format!(
                "{call} addresses the acting player, and this dispatch (a tick system, block \
                 hook, spawn pick, mod_init or mob action) has none; name the player with {twin}"
            ),
        )
    })
}

fn read_only_refusal() -> HostRet {
    HostRet::error(
        ErrorCode::ReadOnly,
        "this host call needs write access to the world, which a read-only dispatch \
         (e.g. a shape placement plan) does not grant"
            .into(),
    )
}

fn no_context() -> HostRet {
    HostRet::error(
        ErrorCode::NoContext,
        "no simulation context is active".into(),
    )
}

pub(in crate::modding) fn read_only_permits(call: &HostCall) -> bool {
    !call.legality().mutates()
}

pub(super) fn sim_call(f: impl FnOnce(&mut SimCtx<'_>)) -> HostRet {
    sim_query(|ctx| {
        f(ctx);
        HostRet::Unit
    })
}

pub(super) fn sim_mutate(f: impl FnOnce(&mut SimCtx<'_>) -> Result<(), HostRet>) -> HostRet {
    sim_query(|ctx| f(ctx).err().unwrap_or(HostRet::Unit))
}

pub(super) fn sim_query(f: impl FnOnce(&mut SimCtx<'_>) -> HostRet) -> HostRet {
    if scope::read_only_active() {
        return read_only_refusal();
    }
    scope::with_active(f).unwrap_or_else(no_context)
}

pub(super) fn sim_read(f: impl FnOnce(&SimCtx<'_>) -> HostRet) -> HostRet {
    scope::with_active_ref(f).unwrap_or_else(no_context)
}

pub(super) fn live_mob<'a>(ctx: &'a SimCtx<'_>, mob_id: u64) -> Option<&'a crate::mob::Instance> {
    ctx.world.mobs().live(mob_id)
}

/// Stream-final gate for WRITE-through-a-cell arms (`SwapBlock`, `ContainerSet`): cell's block, or
/// `Err(Bool(false))` while section is unloaded or streamed content isn't final yet.
/// During that window a plain read lies (shows the base the saved overlay will land on),
/// so an ownership check would see a foreign block and wrongly raise a namespace `Error`.
/// The gated miss is benign, same as any gated read.
pub(super) fn stream_final_cell(ctx: &SimCtx<'_>, pos: IVec3) -> Result<Block, HostRet> {
    ctx.world
        .block_if_stream_final(pos.x, pos.y, pos.z)
        .ok_or(HostRet::Bool(false))
}

pub(super) fn checked_block(block: mod_api::BlockId) -> Result<Block, HostRet> {
    if (block.0 as usize) < Block::all().len() {
        Ok(Block(block.0))
    } else {
        Err(HostRet::invalid(format!(
            "unregistered block id {} (ids are session-scoped; resolve them from your own \
             catalog rows, never persist them)",
            block.0
        )))
    }
}

pub(super) fn finite3(v: [f32; 3], what: &str) -> Result<Vec3, HostRet> {
    if v.iter().all(|c| c.is_finite()) {
        Ok(v.into())
    } else {
        Err(HostRet::invalid(format!("{what}: non-finite component")))
    }
}

pub(in crate::modding) fn finite_pos(
    v: [f64; 3],
    what: &str,
) -> Result<petramond_math::world_pos::WorldPos, HostRet> {
    if v.iter().all(|c| c.is_finite()) {
        Ok(petramond_world::border::clamp(
            petramond_math::world_pos::WorldPos::from_array(v),
        ))
    } else {
        Err(HostRet::invalid(format!("{what}: non-finite component")))
    }
}

pub(super) fn item_by_name(name: &str) -> Option<ItemType> {
    ItemType::by_name(name)
}

pub(super) fn item_name(item: ItemType) -> &'static str {
    petramond_world::registry::names()
        .items
        .name(item.id())
        .unwrap_or("?")
}

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

pub(super) fn abi_data_map(
    what: &str,
    data: &[(String, Vec<u8>)],
) -> Result<petramond_world::item::variant::VariantMap, mod_api::HostRet> {
    use petramond_world::item::variant;
    let mut map = variant::VariantMap::new();
    for (k, v) in data {
        if map.insert(k.clone(), v.clone()).is_some() {
            return Err(mod_api::HostRet::invalid(format!(
                "{what}: duplicate instance-data key '{k}'"
            )));
        }
    }
    if !map.is_empty() && !variant::valid(&map) {
        return Err(mod_api::HostRet::invalid(format!(
            "{what}: invalid instance data (bare key or over-cap map/value)"
        )));
    }
    Ok(map)
}

#[cfg(test)]
mod tests;
