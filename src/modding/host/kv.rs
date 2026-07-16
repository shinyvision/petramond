//! Persistent KV calls: world, per-section-cell, and per-mob surfaces.
//! Writes pass the namespace/size guard; reads cross namespaces.

use mod_api::{HostCall, HostRet};

use crate::mathh::IVec3;

use super::guards::{kv_write_guard, sim_call, sim_query};

/// Phase 3b: persistent KV (world / section-cell / mob surfaces; writes pass
/// [`kv_write_guard`]).
pub(super) fn handle_kv_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::WorldKvGet { key } => {
            sim_query(|ctx| HostRet::Bytes(ctx.world.mod_kv_get(&key).map(<[u8]>::to_vec)))
        }
        HostCall::WorldKvSet { key, value } => match kv_write_guard(mod_id, &key, value.len()) {
            Some(err) => err,
            None => sim_call(|ctx| ctx.world.mod_kv_set(key, value)),
        },
        HostCall::WorldKvDelete { key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| HostRet::Bool(ctx.world.mod_kv_remove(&key))),
        },
        HostCall::SectionKvGet { pos, key } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bytes(
                ctx.world
                    .cell_kv_get(p.x, p.y, p.z, &key)
                    .map(<[u8]>::to_vec),
            )
        }),
        HostCall::SectionKvSet { pos, key, value } => {
            match kv_write_guard(mod_id, &key, value.len()) {
                Some(err) => err,
                None => sim_query(|ctx| {
                    let p = IVec3::from(pos);
                    HostRet::Bool(ctx.world.cell_kv_set(p.x, p.y, p.z, key, value))
                }),
            }
        }
        HostCall::SectionKvDelete { pos, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                let p = IVec3::from(pos);
                HostRet::Bool(ctx.world.cell_kv_remove(p.x, p.y, p.z, &key))
            }),
        },
        HostCall::MobKvGet { mob_index, key } => sim_query(|ctx| {
            HostRet::Bytes(
                ctx.world
                    .mobs()
                    .mod_kv_get(mob_index as usize, &key)
                    .map(<[u8]>::to_vec),
            )
        }),
        HostCall::MobKvSet {
            mob_index,
            key,
            value,
        } => match kv_write_guard(mod_id, &key, value.len()) {
            Some(err) => err,
            None => sim_query(|ctx| {
                HostRet::Bool(
                    ctx.world
                        .mobs_mut()
                        .mod_kv_set(mob_index as usize, key, value),
                )
            }),
        },
        HostCall::MobKvDelete { mob_index, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                HostRet::Bool(ctx.world.mobs_mut().mod_kv_remove(mob_index as usize, &key))
            }),
        },
        other => HostRet::Error(format!(
            "non-KV call {other:?} mis-routed to handle_kv_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::events::{PostQueue, SimCtx};
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::guards::KV_MAX_VALUE_BYTES;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    /// Run `f` with a live SimCtx published, as if inside a guest dispatch.
    fn with_ctx(f: impl FnOnce()) {
        let mut world = World::new(1, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
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
}
