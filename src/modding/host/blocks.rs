//! Block calls: stream-final reads, full-edit-path writes, scheduled
//! ticks, light queries, collision-shape classification, and the
//! model-group swap.

use mod_api::{HostCall, HostRet};

use crate::mathh::IVec3;

use super::guards::{
    batch_guard, checked_block, key_owned_by_namespace, sim_call, sim_query, stream_final_cell,
};

/// Block calls (all sim-scoped, delegating to World).
pub(super) fn handle_block_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SwapModelBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => {
                // Both sides of the swap must be the caller's own blocks: this
                // is a machine flipping ITS placed variant, never a tool for
                // rewriting someone else's content.
                let new_name = crate::registry::names().blocks.name(b.id()).unwrap_or("?");
                if !key_owned_by_namespace(mod_id, new_name) {
                    return HostRet::Error(format!(
                        "SwapModelBlock: block '{new_name}' is not owned by mod '{mod_id}'"
                    ));
                }
                let mod_id = mod_id.to_owned();
                sim_query(move |ctx| {
                    let p = IVec3::from(pos);
                    let old = match stream_final_cell(ctx, p) {
                        Ok(b) => b,
                        Err(miss) => return miss,
                    };
                    let old_name = crate::registry::names()
                        .blocks
                        .name(old.id())
                        .unwrap_or("?");
                    if !key_owned_by_namespace(&mod_id, old_name) {
                        return HostRet::Error(format!(
                            "SwapModelBlock: block '{old_name}' at {pos:?} is not owned by mod \
                             '{mod_id}'"
                        ));
                    }
                    HostRet::Bool(ctx.world.swap_model_block(p, b))
                })
            }
        },
        // Biomes are column-level data fixed at generation (saved overlays
        // never change them), so a loaded-column read cannot lie: no
        // stream-final gate needed.
        HostCall::BiomeAt { pos } => sim_query(move |ctx| {
            HostRet::MaybeByte(ctx.world.biome_at_world(pos[0], pos[1]))
        }),
        // The SURFACE can lie mid-stream (the generated base shows where a
        // saved overlay is about to land), so the found footing must be
        // stream-final like every block read — else a mod builds on terrain
        // the player's save is about to replace.
        HostCall::SurfaceYAt { pos } => sim_query(move |ctx| {
            let y = ctx.world.surface_collision_y(pos[0], pos[1]).filter(|&y| {
                ctx.world.block_if_stream_final(pos[0], y, pos[1]).is_some()
            });
            HostRet::MaybeI32(y)
        }),
        // Mod reads report None ("unloaded") while a section's streamed
        // content is not final — a half-streamed read would show the
        // generated base where the player's saved record is about to land.
        HostCall::GetBlock { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Block(
                ctx.world
                    .block_if_stream_final(p.x, p.y, p.z)
                    .map(|b| mod_api::BlockId(b.id())),
            )
        }),
        HostCall::GetBlocks { positions } => {
            if let Some(err) = batch_guard("GetBlocks position", positions.len()) {
                return err;
            }
            sim_query(|ctx| {
                HostRet::Blocks(
                    positions
                        .iter()
                        .map(|&pos| {
                            let p = IVec3::from(pos);
                            ctx.world
                                .block_if_stream_final(p.x, p.y, p.z)
                                .map(|b| mod_api::BlockId(b.id()))
                        })
                        .collect(),
                )
            })
        }
        HostCall::SetBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => sim_query(|ctx| {
                let p = IVec3::from(pos);
                HostRet::Bool(ctx.world.set_block_world(p.x, p.y, p.z, b))
            }),
        },
        HostCall::SetBlocks { blocks } => {
            if let Some(err) = batch_guard("SetBlocks write", blocks.len()) {
                return err;
            }
            sim_query(|ctx| {
                let mut set = 0u64;
                for &(pos, block) in &blocks {
                    let Ok(b) = checked_block(block) else {
                        return HostRet::Error(format!(
                            "SetBlocks: unregistered block id {}",
                            block.0
                        ));
                    };
                    let p = IVec3::from(pos);
                    if ctx.world.set_block_world(p.x, p.y, p.z, b) {
                        set += 1;
                    }
                }
                HostRet::U64(set)
            })
        }
        HostCall::ScheduleTick { pos, delay } => {
            sim_call(|ctx| ctx.world.schedule_tick(pos.into(), delay))
        }
        HostCall::IsLoaded { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bool(ctx.world.section_stream_final_at(p.x, p.y, p.z))
        }),
        // Light reads follow the GetBlock contract: the engine's own light
        // accessors fall back to "open sky / no block light" for absent
        // sections (the mesh-border fallback), which for a MOD read is a
        // fabricated value light-driven policy would act on — gate on
        // stream finality and answer `None` instead.
        HostCall::LightAt { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Light(ctx.world.block_if_stream_final(p.x, p.y, p.z).map(|_| {
                mod_api::LightData {
                    combined: ctx.world.combined_light6_at_world(p.x, p.y, p.z),
                    sky: ctx.world.skylight6_at_world(p.x, p.y, p.z),
                    block: ctx.world.blocklight6_at_world(p.x, p.y, p.z),
                }
            }))
        }),
        HostCall::CollisionShapeAt { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::CollisionShape(ctx.world.block_if_stream_final(p.x, p.y, p.z).map(|_| {
                match ctx.world.collision_shape_class(p.x, p.y, p.z) {
                    crate::world::CollisionShapeClass::Empty => mod_api::CollisionShape::Empty,
                    crate::world::CollisionShapeClass::Partial => mod_api::CollisionShape::Partial,
                    crate::world::CollisionShapeClass::Full => mod_api::CollisionShape::Full,
                }
            }))
        }),
        other => HostRet::Error(format!(
            "non-block call {other:?} mis-routed to handle_block_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{CollisionShape, HostCall, HostRet};

    use crate::block::Block;
    use crate::chunk::ChunkPos;
    use crate::events::{PostQueue, SimCtx};
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::guards::SIM_BATCH_MAX;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    /// Publish a SimCtx over `world` and run `f`, as if inside a dispatch.
    fn with_world_ctx(world: &mut World, f: impl FnOnce()) {
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, f);
    }

    /// Batched sim/registry calls are hard-capped at [`SIM_BATCH_MAX`]
    /// elements: the watchdog charges guest compute only, so without the cap
    /// one maximal batch is unmetered host work that stalls the sim. Over-cap
    /// = `Error` (mod bug, loud); at-cap batches still answer.
    #[test]
    fn batched_calls_reject_oversized_batches() {
        let mut store = ModStoreData::new("alpha", 1);
        // The guard fires before any sim access, so over-cap is rejected as
        // the CAP error even outside a dispatch scope.
        for (name, call) in [
            (
                "GetBlocks",
                HostCall::GetBlocks {
                    positions: vec![[0, 0, 0]; SIM_BATCH_MAX + 1],
                },
            ),
            (
                "SetBlocks",
                HostCall::SetBlocks {
                    blocks: vec![([0, 0, 0], mod_api::BlockId(0)); SIM_BATCH_MAX + 1],
                },
            ),
            (
                "ContainerGetMany",
                HostCall::ContainerGetMany {
                    positions: vec![[0, 0, 0]; SIM_BATCH_MAX + 1],
                },
            ),
            (
                "ContainerSet",
                HostCall::ContainerSet {
                    pos: [0, 0, 0],
                    slots: vec![(0, None); SIM_BATCH_MAX + 1],
                },
            ),
            (
                "ItemNames",
                HostCall::ItemNames {
                    items: vec![mod_api::ItemId(0); SIM_BATCH_MAX + 1],
                },
            ),
        ] {
            match handle_host_call(&mut store, call) {
                HostRet::Error(e) => assert!(
                    e.contains("exceeds"),
                    "{name}: expected the cap error, got '{e}'"
                ),
                other => panic!("{name}: over-cap batch answered {other:?}"),
            }
        }
        // An at-cap batch is served (registry lane needs no sim scope).
        let got = handle_host_call(
            &mut store,
            HostCall::ItemNames {
                items: vec![mod_api::ItemId(0); SIM_BATCH_MAX],
            },
        );
        assert!(matches!(got, HostRet::Names(v) if v.len() == SIM_BATCH_MAX));
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        with_world_ctx(&mut world, || {
            let got = handle_host_call(
                &mut store,
                HostCall::GetBlocks {
                    positions: vec![[8, 64, 8]; SIM_BATCH_MAX],
                },
            );
            assert!(matches!(got, HostRet::Blocks(v) if v.len() == SIM_BATCH_MAX));
        });
    }

    /// `LightAt` follows the block-read contract: an unloaded (or not yet
    /// stream-final) cell answers `None` — never the engine's open-sky
    /// fallback — so light-driven policy cannot act on fabricated values.
    #[test]
    fn light_at_answers_none_for_unloaded_cells() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        with_world_ctx(&mut world, || {
            let loaded = handle_host_call(&mut store, HostCall::LightAt { pos: [8, 64, 8] });
            assert!(
                matches!(loaded, HostRet::Light(Some(_))),
                "loaded cell must answer light, got {loaded:?}"
            );
            let unloaded =
                handle_host_call(&mut store, HostCall::LightAt { pos: [512, 64, 512] });
            assert_eq!(unloaded, HostRet::Light(None));
        });
    }

    /// `CollisionShapeAt` is generic geometry: one full unit cube = `Full`,
    /// stairs = `Partial`, air and water = `Empty` (which is why footing
    /// policy needs its own water check), unloaded = `None`.
    #[test]
    fn collision_shape_classifies_geometry_and_gates_unloaded() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(world.set_block_world(8, 63, 8, Block::Stone));
        assert!(world.set_block_world(8, 64, 8, Block::OakStairs));
        assert!(world.set_block_world(8, 65, 8, Block::Water));
        with_world_ctx(&mut world, || {
            let mut shape = |pos| match handle_host_call(&mut store, HostCall::CollisionShapeAt { pos })
            {
                HostRet::CollisionShape(s) => s,
                other => panic!("expected a shape reply, got {other:?}"),
            };
            assert_eq!(shape([8, 63, 8]), Some(CollisionShape::Full));
            assert_eq!(shape([8, 64, 8]), Some(CollisionShape::Partial));
            assert_eq!(shape([8, 65, 8]), Some(CollisionShape::Empty));
            assert_eq!(shape([8, 66, 8]), Some(CollisionShape::Empty), "air");
            assert_eq!(shape([512, 64, 512]), None, "unloaded gates like GetBlock");
        });
    }
}
