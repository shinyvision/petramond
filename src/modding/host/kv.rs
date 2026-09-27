use mod_api::{ErrorCode, HostRet, KvCall};

use petramond_math::math::IVec3;

use super::guards::{batch_guard, kv_write_guard, sim_call, sim_query, sim_read, CELL_KV_MAX_KEYS};

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

pub(super) fn handle_kv_call(mod_id: &str, call: KvCall) -> HostRet {
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
            sim_call(|ctx| ctx.world.world_kv_set(key, value))
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
