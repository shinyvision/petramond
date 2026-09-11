//! Persistent KV calls: world and per-section-cell surfaces (per-mob keyed
//! data rides the typed TAG map — see `tags.rs`).
//! Writes pass the namespace/size guard; reads cross namespaces.

use mod_api::{HostCall, HostRet};

use petramond_math::math::IVec3;

use super::guards::{batch_guard, kv_write_guard, sim_call, sim_query, CELL_KV_MAX_KEYS};

/// Run one KV write behind [`kv_write_guard`], handing the key back to the
/// operation when the guard passes (deletes guard with `value_len` 0).
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

/// Persistent-KV calls (world / section-cell surfaces; writes pass
/// [`kv_write_guard`]).
pub(super) fn handle_kv_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SectionKvFind { section, key } => sim_query(|ctx| {
            use petramond_world::chunk::SectionPos;
            let Some(origin) = section
                .into_iter()
                .map(|n| n.checked_mul(16))
                .collect::<Option<Vec<_>>>()
            else {
                return HostRet::Error("section coordinates overflow".into());
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
            let Some(section) = ctx.world.section_ref(pos) else {
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
        HostCall::WorldKvGet { key } => {
            sim_query(|ctx| HostRet::Bytes(ctx.world.world_kv_get(&key).map(<[u8]>::to_vec)))
        }
        HostCall::WorldKvSet { key, value } => guarded_write(mod_id, key, value.len(), |key| {
            sim_call(|ctx| ctx.world.world_kv_set(key, value))
        }),
        HostCall::WorldKvDelete { key } => guarded_write(mod_id, key, 0, |key| {
            sim_query(|ctx| HostRet::Bool(ctx.world.world_kv_remove(&key)))
        }),
        HostCall::SectionKvGet { pos, key } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bytes(
                ctx.world
                    .cell_kv_get(p.x, p.y, p.z, &key)
                    .map(<[u8]>::to_vec),
            )
        }),
        HostCall::SectionKvSet { pos, key, value } => {
            guarded_write(mod_id, key, value.len(), |key| {
                sim_query(|ctx| {
                    let p = IVec3::from(pos);
                    // Aggregate cap: a NEW key on a cell already at the limit
                    // is an error (overwrites always pass) — see
                    // `CELL_KV_MAX_KEYS` for why cells must stay small.
                    if ctx.world.cell_kv_get(p.x, p.y, p.z, &key).is_none()
                        && ctx.world.cell_kv_count(p.x, p.y, p.z) >= CELL_KV_MAX_KEYS
                    {
                        return HostRet::Error(format!(
                            "cell {p:?} already holds {CELL_KV_MAX_KEYS} KV keys"
                        ));
                    }
                    HostRet::Bool(ctx.world.cell_kv_set(p.x, p.y, p.z, key, value))
                })
            })
        }
        HostCall::SectionKvDelete { pos, key } => guarded_write(mod_id, key, 0, |key| {
            sim_query(|ctx| {
                let p = IVec3::from(pos);
                HostRet::Bool(ctx.world.cell_kv_remove(p.x, p.y, p.z, &key))
            })
        }),
        // ONE key across many cells: the shape a machine KIND reads and writes
        // its state in, and the reason it exists is that the per-cell form
        // made a mod's tick cost one crossing per placed machine.
        HostCall::SectionKvGetMany { key, positions } => {
            if let Some(err) = batch_guard("SectionKvGetMany position", positions.len()) {
                return err;
            }
            sim_query(move |ctx| {
                HostRet::BytesMany(
                    positions
                        .into_iter()
                        .map(|pos| {
                            let p = IVec3::from(pos);
                            ctx.world
                                .cell_kv_get(p.x, p.y, p.z, &key)
                                .map(<[u8]>::to_vec)
                        })
                        .collect(),
                )
            })
        }
        HostCall::SectionKvSetMany { key, writes } => {
            if let Some(err) = batch_guard("SectionKvSetMany write", writes.len()) {
                return err;
            }
            // The value guard runs against the LARGEST write, so one oversized
            // value fails the whole call exactly as it would alone.
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
                                if ctx.world.cell_kv_get(p.x, p.y, p.z, &key).is_none()
                                    && ctx.world.cell_kv_count(p.x, p.y, p.z) >= CELL_KV_MAX_KEYS
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
        other => HostRet::Error(format!(
            "non-KV call {other:?} mis-routed to handle_kv_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests;
