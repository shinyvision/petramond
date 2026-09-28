use std::ops::Bound;

use mod_api::{ErrorCode, HostRet, KvCall};

use petramond_math::math::IVec3;

use crate::modding::health::ModHealth;

use super::guards::{batch_guard, kv_write_guard, sim_query, sim_read, CELL_KV_MAX_KEYS};

fn guarded_write(
    mod_id: &str,
    key: String,
    value_len: usize,
    op: impl FnOnce(String) -> HostRet,
) -> HostRet {
    match kv_write_guard(mod_id, &key, value_len) {
        Some(err) => err,
        None => op(key),
    }
}

/// The namespace a world KV write lands in, with the bytes (keys plus values) the namespace holds
/// before it and would hold after it. World KV is saved with the level, so this is what the
/// mod's watchdog lets grow gradually.
fn namespace_usage<'k>(
    kv: &std::collections::BTreeMap<String, Vec<u8>>,
    key: &'k str,
    value_len: usize,
) -> Option<(&'k str, u64, u64)> {
    let prefix = &key[..=key.find(':')?];
    let (mut before, mut replaced) = (0u64, 0u64);
    for (k, v) in kv
        .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
        .take_while(|(k, _)| k.starts_with(prefix))
    {
        let size = (k.len() + v.len()) as u64;
        before += size;
        if k == key {
            replaced = size;
        }
    }
    let after = before - replaced + (key.len() + value_len) as u64;
    Some((&prefix[..prefix.len() - 1], before, after))
}

pub(super) fn handle_kv_call(mod_id: &str, health: &ModHealth, call: KvCall) -> HostRet {
    match call {
        KvCall::SectionKvFind { section, key } => sim_read(|ctx| {
            use petramond_world::chunk::SectionPos;
            let Some(origin) = section
                .into_iter()
                .map(|n| n.checked_mul(16))
                .collect::<Option<Vec<_>>>()
            else {
                return HostRet::invalid("section coordinates overflow".into());
            };
            if !ctx
                .world
                .section_stream_final_at(origin[0], origin[1], origin[2])
            {
                return HostRet::FoundBlocks(None);
            }
            let Some(pos) = SectionPos::from_world(origin[0], origin[1], origin[2]) else {
                return HostRet::FoundBlocks(Some(Vec::new()));
            };
            let Some(section) = ctx.world.data().section_ref(pos) else {
                return HostRet::FoundBlocks(Some(Vec::new()));
            };
            let mut indices: Vec<_> = section
                .cell_kv()
                .iter()
                .filter_map(|(&idx, data)| data.contains_key(&key).then_some(idx))
                .collect();
            indices.sort_unstable();
            HostRet::FoundBlocks(Some(
                indices
                    .into_iter()
                    .map(|idx| {
                        [
                            origin[0] + i32::from(idx % 16),
                            origin[1] + i32::from(idx / 256),
                            origin[2] + i32::from((idx / 16) % 16),
                        ]
                    })
                    .collect(),
            ))
        }),
        KvCall::WorldKvGet { key } => {
            sim_read(|ctx| HostRet::Bytes(ctx.world.data().world_kv_get(&key).map(<[u8]>::to_vec)))
        }
        KvCall::WorldKvSet { key, value } => guarded_write(mod_id, key, value.len(), |key| {
            sim_query(|ctx| {
                let usage = namespace_usage(ctx.world.data().world_kv(), &key, value.len());
                if let Some((namespace, before, after)) = usage {
                    if let Err(why) = health.watchdog().grow_storage(namespace, before, after) {
                        health.disable(&why);
                        return HostRet::error(ErrorCode::LimitExceeded, why);
                    }
                }
                ctx.world.world_kv_set(key, value);
                HostRet::Unit
            })
        }),
        KvCall::WorldKvDelete { key } => guarded_write(mod_id, key, 0, |key| {
            sim_query(|ctx| HostRet::Bool(ctx.world.world_kv_remove(&key)))
        }),
        KvCall::SectionKvGet { pos, key } => sim_read(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bytes(
                ctx.world
                    .data()
                    .cell_kv_get(p.x, p.y, p.z, &key)
                    .map(<[u8]>::to_vec),
            )
        }),
        KvCall::SectionKvSet { pos, key, value } => {
            guarded_write(mod_id, key, value.len(), |key| {
                sim_query(|ctx| {
                    let p = IVec3::from(pos);
                    if ctx.world.data().cell_kv_get(p.x, p.y, p.z, &key).is_none()
                        && ctx.world.data().cell_kv_count(p.x, p.y, p.z) >= CELL_KV_MAX_KEYS
                    {
                        return HostRet::error(
                            ErrorCode::LimitExceeded,
                            format!("cell {p:?} already holds {CELL_KV_MAX_KEYS} KV keys"),
                        );
                    }
                    HostRet::Bool(ctx.world.cell_kv_set(p.x, p.y, p.z, key, value))
                })
            })
        }
        KvCall::SectionKvDelete { pos, key } => guarded_write(mod_id, key, 0, |key| {
            sim_query(|ctx| {
                let p = IVec3::from(pos);
                HostRet::Bool(ctx.world.cell_kv_remove(p.x, p.y, p.z, &key))
            })
        }),
        KvCall::SectionKvGetMany { key, positions } => {
            if let Some(err) = batch_guard("SectionKvGetMany position", positions.len()) {
                return err;
            }
            sim_read(move |ctx| {
                HostRet::BytesMany(
                    positions
                        .into_iter()
                        .map(|pos| {
                            let p = IVec3::from(pos);
                            ctx.world
                                .data()
                                .cell_kv_get(p.x, p.y, p.z, &key)
                                .map(<[u8]>::to_vec)
                        })
                        .collect(),
                )
            })
        }
        KvCall::SectionKvSetMany { key, writes } => {
            if let Some(err) = batch_guard("SectionKvSetMany write", writes.len()) {
                return err;
            }
            let widest = writes
                .iter()
                .map(|(_, v)| v.as_ref().map_or(0, Vec::len))
                .max()
                .unwrap_or(0);
            guarded_write(mod_id, key, widest, |key| {
                sim_query(move |ctx| {
                    HostRet::Bools(
                        writes
                            .into_iter()
                            .map(|(pos, value)| {
                                let p = IVec3::from(pos);
                                let Some(value) = value else {
                                    return ctx.world.cell_kv_remove(p.x, p.y, p.z, &key);
                                };
                                if ctx.world.data().cell_kv_get(p.x, p.y, p.z, &key).is_none()
                                    && ctx.world.data().cell_kv_count(p.x, p.y, p.z)
                                        >= CELL_KV_MAX_KEYS
                                {
                                    return false;
                                }
                                ctx.world.cell_kv_set(p.x, p.y, p.z, key.clone(), value)
                            })
                            .collect(),
                    )
                })
            })
        }
    }
}

#[cfg(test)]
mod tests;
