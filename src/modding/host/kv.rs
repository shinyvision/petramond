//! Persistent KV calls: world and per-section-cell surfaces (per-mob keyed
//! data rides the typed TAG map — see `tags.rs`).
//! Writes pass the namespace/size guard; reads cross namespaces.

use mod_api::{HostCall, HostRet};

use crate::mathh::IVec3;

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
        HostCall::WorldKvGet { key } => {
            sim_query(|ctx| HostRet::Bytes(ctx.world.mod_kv_get(&key).map(<[u8]>::to_vec)))
        }
        HostCall::WorldKvSet { key, value } => guarded_write(mod_id, key, value.len(), |key| {
            sim_call(|ctx| ctx.world.mod_kv_set(key, value))
        }),
        HostCall::WorldKvDelete { key } => guarded_write(mod_id, key, 0, |key| {
            sim_query(|ctx| HostRet::Bool(ctx.world.mod_kv_remove(&key)))
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
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::events::{PostQueue, SimCtx};
    use crate::events::tick::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::guards::KV_MAX_VALUE_BYTES;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    /// Run `f` with a live SimCtx published, as if inside a guest dispatch.
    /// The world gets one flat-floored loaded chunk so section-cell KV writes
    /// have a writable target.
    fn with_ctx(f: impl FnOnce()) {
        let mut world = World::new(1, 1);
        let mut c = crate::chunk::Chunk::new(0, 0);
        for z in 0..crate::chunk::CHUNK_SZ {
            for x in 0..crate::chunk::CHUNK_SX {
                c.set_block(x, 64, z, crate::block::Block::Stone);
            }
        }
        world.insert_chunk_for_test(crate::chunk::ChunkPos::new(0, 0), c);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui_state::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, f);
    }

    /// The KV namespace contract: writes must carry the CALLER's own
    /// `mod_id:` prefix or an engine-owned `petramond:` key (foreign and bare keys
    /// are rejected with an error), while reads may cross namespaces — that
    /// asymmetry IS the cross-mod interop surface. Size caps reject oversized
    /// values.
    #[test]
    fn kv_writes_enforce_own_namespace_and_reads_cross() {
        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        with_ctx(|| {
            // Own-prefix write lands.
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvSet {
                        key: "alpha:x".into(),
                        value: vec![7],
                    },
                ),
                HostRet::Unit
            );
            // Engine-owned public surfaces are intentionally writable.
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvSet {
                        key: "petramond:time".into(),
                        value: vec![1],
                    },
                ),
                HostRet::Unit
            );
            // A foreign-prefix write is rejected...
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvSet {
                        key: "alpha:x".into(),
                        value: vec![9],
                    },
                ),
                HostRet::Error(_)
            ));
            // ...and so are bare / degenerate keys.
            for bad in ["x", "alpha:", "petramond:", "alphax:y", "beta"] {
                assert!(
                    matches!(
                        handle_host_call(
                            &mut beta,
                            HostCall::WorldKvSet {
                                key: bad.into(),
                                value: vec![1],
                            },
                        ),
                        HostRet::Error(_)
                    ),
                    "write with key '{bad}' must be rejected"
                );
            }
            // The rejected write changed nothing; a cross-namespace READ works.
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvGet {
                        key: "alpha:x".into(),
                    },
                ),
                HostRet::Bytes(Some(vec![7]))
            );
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvGet {
                        key: "petramond:time".into(),
                    },
                ),
                HostRet::Bytes(Some(vec![1]))
            );
            // Deletes are writes: foreign rejected, own applies.
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvDelete {
                        key: "alpha:x".into(),
                    },
                ),
                HostRet::Error(_)
            ));
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvDelete {
                        key: "alpha:x".into(),
                    },
                ),
                HostRet::Bool(true)
            );
            // The value size cap holds (same guard on every KV write surface).
            assert!(matches!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvSet {
                        key: "alpha:big".into(),
                        value: vec![0; KV_MAX_VALUE_BYTES + 1],
                    },
                ),
                HostRet::Error(_)
            ));
        });
        // Outside any dispatch scope, sim-touching KV calls are rejected.
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::WorldKvGet {
                    key: "alpha:x".into(),
                },
            ),
            HostRet::Error(_)
        ));
    }

    /// The per-cell AGGREGATE cap: one more DISTINCT key on a cell already
    /// holding `CELL_KV_MAX_KEYS` errors, while overwriting an existing key
    /// at the cap passes (the cap bounds the map, not writes) and removing a
    /// key frees a slot. The cap is what keeps every `BlockDelta` — which
    /// ships the cell's whole KV map — a bounded wire payload.
    #[test]
    fn section_kv_caps_distinct_keys_per_cell() {
        use crate::modding::host::guards::CELL_KV_MAX_KEYS;
        let mut alpha = ModStoreData::new("alpha", 1);
        with_ctx(|| {
            let pos = [2, 65, 2];
            let set = |m: &mut ModStoreData, key: String| {
                handle_host_call(
                    m,
                    HostCall::SectionKvSet {
                        pos,
                        key,
                        value: vec![1],
                    },
                )
            };
            for i in 0..CELL_KV_MAX_KEYS {
                assert_eq!(
                    set(&mut alpha, format!("alpha:k{i}")),
                    HostRet::Bool(true),
                    "key {i} fits under the cap"
                );
            }
            assert!(
                matches!(
                    set(&mut alpha, "alpha:one_too_many".into()),
                    HostRet::Error(_)
                ),
                "a new key beyond the cap is rejected"
            );
            assert_eq!(
                set(&mut alpha, "alpha:k0".into()),
                HostRet::Bool(true),
                "overwriting an existing key at the cap passes"
            );
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::SectionKvDelete {
                        pos,
                        key: "alpha:k1".into(),
                    },
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                set(&mut alpha, "alpha:one_too_many".into()),
                HostRet::Bool(true),
                "a removal frees a slot"
            );
        });
    }
}
