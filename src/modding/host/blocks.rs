//! Block calls: stream-final reads, full-edit-path writes, scheduled
//! ticks, light queries, collision-shape classification, and the
//! model-group swap.

use mod_api::{HostCall, HostRet};

use petramond_math::math::IVec3;

use super::guards::{
    batch_guard, checked_block, finite3, key_owned_by_namespace, sim_call, sim_query,
    stream_final_cell,
};

/// The three presentation WRITES below all ask the same question first: does
/// the caller OWN the placed block it is addressing? A mod dresses ITS OWN
/// machine, never someone else's block — and none of them may act on a cell
/// whose streaming state is not final.
///
/// `call` names the caller only so the error says which one refused.
fn owned_block_at(
    ctx: &mut crate::modding::SimCtx<'_>,
    mod_id: &str,
    pos: [i32; 3],
    call: &str,
) -> Result<petramond_world::block::Block, HostRet> {
    let p = IVec3::from(pos);
    let block = stream_final_cell(ctx, p)?;
    let name = petramond_world::registry::names()
        .blocks
        .name(block.id())
        .unwrap_or("?");
    if !key_owned_by_namespace(mod_id, name) {
        return Err(HostRet::Error(format!(
            "{call}: block '{name}' at {pos:?} is not owned by mod '{mod_id}'"
        )));
    }
    Ok(block)
}

/// Whether every float a draw prim carries is finite.
///
/// A non-finite one is a loud mod bug rather than a dropped box, and the reason
/// is the IDEMPOTENCE gate: a machine resubmits its set every tick and the
/// engine drops an unchanged submission by comparing the submitted prims, but
/// `NaN != NaN` — so one NaN corner makes every tick a change, replicating a
/// set that (having been dropped at resolve) draws nothing at all. Refusing it
/// here is what keeps that comparison total.
fn draw_prim_finite(prim: &mod_api::DrawPrim) -> bool {
    match prim {
        mod_api::DrawPrim::Cuboid { min, max, .. } => min.iter().chain(max).all(|v| v.is_finite()),
        mod_api::DrawPrim::Item {
            at,
            scale,
            yaw,
            pitch,
            ..
        } => at.iter().chain([scale, yaw, pitch]).all(|v| v.is_finite()),
    }
}

/// The per-set validation both draw calls run: the prim cap and finiteness.
/// `what` names the caller for the error line.
fn check_draw_set(what: &str, prims: &[mod_api::DrawPrim]) -> Option<HostRet> {
    const MAX: usize = mod_api::DRAW_PRIMS_MAX;
    if prims.len() > MAX {
        return Some(HostRet::Error(format!(
            "{what}: {} prims; the cap is {MAX}",
            prims.len()
        )));
    }
    let bad = prims.iter().position(|p| !draw_prim_finite(p))?;
    Some(HostRet::Error(format!(
        "{what}: prim {bad} has a non-finite component"
    )))
}

/// Block calls (all sim-scoped, delegating to World).
pub(super) fn handle_block_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        // The batched presentation writes. They exist because a mod's tick
        // cost must not scale with how many machines the player has built:
        // one crossing for the whole kind, not one per placed block.
        //
        // The per-SET checks (prim cap, finiteness) fail the WHOLE call like
        // the single form: they are malformed input, and a mod that sends one
        // is broken. Per-CELL outcomes — unloaded, or a block that stopped
        // being this mod's — answer `false` in the parallel reply instead,
        // where the single form errors: submitting one machine that was
        // broken this tick is the normal way to lose a race, and taking the
        // pack down for it would make the batched form unusable.
        // Pinned by `a_batched_draw_answers_per_entry_where_the_single_call_errors`.
        HostCall::SetBlockDraws { sets } => {
            if let Some(err) = batch_guard("SetBlockDraws set", sets.len()) {
                return err;
            }
            for (_, prims) in &sets {
                if let Some(err) = check_draw_set("SetBlockDraws", prims) {
                    return err;
                }
            }
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                HostRet::Bools(
                    sets.into_iter()
                        .map(|(pos, prims)| {
                            if owned_block_at(ctx, &mod_id, pos, "SetBlockDraws").is_err() {
                                return false;
                            }
                            ctx.world.set_block_draw(IVec3::from(pos), prims.into());
                            true
                        })
                        .collect(),
                )
            })
        }
        HostCall::SetModelPartsMany { sets } => {
            if let Some(err) = batch_guard("SetModelPartsMany set", sets.len()) {
                return err;
            }
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                HostRet::Bools(
                    sets.into_iter()
                        .map(|(pos, parts, tint)| {
                            owned_block_at(ctx, &mod_id, pos, "SetModelPartsMany").is_ok()
                                && ctx.world.set_model_parts(IVec3::from(pos), parts, tint)
                        })
                        .collect(),
                )
            })
        }
        HostCall::SetBlockDraw { pos, prims } => {
            if let Some(err) = check_draw_set("SetBlockDraw", &prims) {
                return err;
            }
            let mod_id = mod_id.to_owned();
            sim_query(
                move |ctx| match owned_block_at(ctx, &mod_id, pos, "SetBlockDraw") {
                    Err(e) => e,
                    Ok(_) => {
                        ctx.world.set_block_draw(IVec3::from(pos), prims.into());
                        // TRUE = the submission was accepted, which an empty
                        // (clearing) one is: a mod checking the reply must not
                        // read its own clear as a refusal. `false` means
                        // UNLOADED and nothing else — a foreign block already
                        // left through the `Err` arm above.
                        HostRet::Bool(true)
                    }
                },
            )
        }
        HostCall::SetModelParts { pos, parts, tint } => {
            let mod_id = mod_id.to_owned();
            sim_query(
                move |ctx| match owned_block_at(ctx, &mod_id, pos, "SetModelParts") {
                    Err(e) => e,
                    Ok(_) => {
                        HostRet::Bool(ctx.world.set_model_parts(IVec3::from(pos), parts, tint))
                    }
                },
            )
        }
        // A READ of the same space `SetBlockDraw` writes in, so it needs no
        // ownership check: knowing where another mod's spout points is no more
        // than `GetBlock` already tells you.
        HostCall::BlockLocalToWorld { pos, points } => {
            if let Some(err) = batch_guard("BlockLocalToWorld point", points.len()) {
                return err;
            }
            sim_query(move |ctx| {
                let p = IVec3::from(pos);
                // Gated like every other mod read: mid-stream the cell shows
                // the generated base where a saved overlay is about to land,
                // and a machine's facing is exactly what that overlay carries.
                if stream_final_cell(ctx, p).is_err() {
                    return HostRet::Points(None);
                }
                let to_world = ctx.world.block_local_transform(p);
                HostRet::Points(Some(
                    points
                        .iter()
                        .map(|&q| {
                            to_world
                                .transform_point3(petramond_math::math::Vec3::from(q))
                                .to_array()
                        })
                        .collect(),
                ))
            })
        }
        HostCall::SwapModelBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => {
                // BOTH sides must be the caller's own: this is a machine
                // flipping ITS placed variant, never a tool for rewriting
                // someone else's content. The destination is checked here
                // because it needs no world read.
                let new_name = petramond_world::registry::names()
                    .blocks
                    .name(b.id())
                    .unwrap_or("?");
                if !key_owned_by_namespace(mod_id, new_name) {
                    return HostRet::Error(format!(
                        "SwapModelBlock: block '{new_name}' is not owned by mod '{mod_id}'"
                    ));
                }
                let mod_id = mod_id.to_owned();
                sim_query(
                    move |ctx| match owned_block_at(ctx, &mod_id, pos, "SwapModelBlock") {
                        Err(e) => e,
                        Ok(_) => HostRet::Bool(ctx.world.swap_model_block(IVec3::from(pos), b)),
                    },
                )
            }
        },
        // Biomes are column-level data fixed at generation (saved overlays
        // never change them), so a loaded-column read cannot lie: no
        // stream-final gate needed.
        HostCall::BiomeAt { pos } => {
            sim_query(move |ctx| HostRet::MaybeByte(ctx.world.biome_at_world(pos[0], pos[1])))
        }
        // The SURFACE can lie mid-stream (the generated base shows where a
        // saved overlay is about to land), so the found footing must be
        // stream-final like every block read — else a mod builds on terrain
        // the player's save is about to replace.
        HostCall::SurfaceYAt { pos } => sim_query(move |ctx| {
            let y = ctx
                .world
                .surface_collision_y(pos[0], pos[1])
                .filter(|&y| ctx.world.block_if_stream_final(pos[0], y, pos[1]).is_some());
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
        HostCall::Raycast {
            from,
            dir,
            max,
            filter,
        } => {
            let from = match finite3(from, "Raycast.from") {
                Ok(v) => v,
                Err(e) => return e,
            };
            let dir = match finite3(dir, "Raycast.dir") {
                Ok(v) if v.length_squared() > f32::EPSILON => v.normalize(),
                Ok(_) => return HostRet::Error("Raycast: zero direction".into()),
                Err(e) => return e,
            };
            if !max.is_finite() || max <= 0.0 || max > mod_api::RAYCAST_MAX_DISTANCE {
                return HostRet::Error(format!(
                    "Raycast: max must be finite and in (0, {}]",
                    mod_api::RAYCAST_MAX_DISTANCE
                ));
            }
            let filter = match filter {
                mod_api::RayFilter::Selectable => crate::player::RayFilter::Selectable,
                mod_api::RayFilter::Collidable => crate::player::RayFilter::Collidable,
            };
            sim_query(move |ctx| {
                HostRet::Raycast(
                    crate::player::Player::raycast_filtered(from, dir, max, filter, ctx.world).map(
                        |(hit, distance)| mod_api::RaycastHitData {
                            block: hit.block.to_array(),
                            face: hit.normal.to_array(),
                            distance,
                        },
                    ),
                )
            })
        }
        HostCall::FindBlocks { min, max, blocks } => {
            if let Some(err) = batch_guard("FindBlocks block", blocks.len()) {
                return err;
            }
            if min.iter().zip(&max).any(|(lo, hi)| lo > hi) {
                return HostRet::Error(format!("FindBlocks: inverted box {min:?}..{max:?}"));
            }
            let volume = min
                .iter()
                .zip(&max)
                .map(|(lo, hi)| (hi - lo) as i64 + 1)
                .product::<i64>();
            if volume > super::guards::FIND_BLOCKS_VOLUME_MAX {
                return HostRet::Error(format!(
                    "FindBlocks: box volume {volume} exceeds {}",
                    super::guards::FIND_BLOCKS_VOLUME_MAX
                ));
            }
            let mut wanted = Vec::with_capacity(blocks.len());
            for &b in &blocks {
                match checked_block(b) {
                    Ok(block) => wanted.push(block),
                    Err(e) => return e,
                }
            }
            sim_query(move |ctx| {
                let mut found = Vec::new();
                // Cells outside the world's vertical range are definitionally
                // empty — they can never match, and treating them as
                // unreadable would starve every search near the world's top
                // or bottom. Clamp instead of gating.
                let y_lo = min[1].max(petramond_world::chunk::WORLD_MIN_Y);
                let y_hi = max[1].min(petramond_world::chunk::WORLD_MAX_Y - 1);
                // Scan order (y, then z, then x ascending) is the documented
                // ABI contract — deterministic for every caller.
                for y in y_lo..=y_hi {
                    for z in min[2]..=max[2] {
                        for x in min[0]..=max[0] {
                            let Some(block) = ctx.world.block_if_stream_final(x, y, z) else {
                                return HostRet::FoundBlocks(None);
                            };
                            if wanted.contains(&block) {
                                found.push([x, y, z]);
                            }
                        }
                    }
                }
                HostRet::FoundBlocks(Some(found))
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
                    block_rgb: ctx.world.blocklight6_rgb_at_world(p.x, p.y, p.z),
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

    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, SimCtx};
    use crate::modding::host::guards::SIM_BATCH_MAX;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;
    use petramond_math::math::Vec3;
    use petramond_world::block::Block;
    use petramond_world::chunk::ChunkPos;

    /// Publish a SimCtx over `world` and run `f`, as if inside a dispatch.
    fn with_world_ctx(world: &mut World, f: impl FnOnce()) {
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();
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

    /// The two ray filters answer two different questions about the same
    /// cells: the crosshair's rule stops on a plant's selection box, a
    /// body's rule passes it and stops on the solid behind — and the
    /// distance answers where. A ray past `max` or with nothing in it
    /// answers `None`; a malformed request is an error, never a fabricated
    /// miss.
    #[test]
    fn raycast_filters_stop_on_what_they_say_and_report_the_distance() {
        use petramond_world::block::Block;
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.set_block_world(4, 64, 8, Block::Poppy);
        world.set_block_world(7, 64, 8, Block::Stone);
        let cast = |store: &mut ModStoreData, max: f32, filter: mod_api::RayFilter| {
            handle_host_call(
                store,
                HostCall::Raycast {
                    from: [1.5, 64.2, 8.5],
                    dir: [2.0, 0.0, 0.0],
                    max,
                    filter,
                },
            )
        };
        with_world_ctx(&mut world, || {
            let HostRet::Raycast(Some(plant)) =
                cast(&mut store, 10.0, mod_api::RayFilter::Selectable)
            else {
                panic!("the crosshair's ray selects the plant");
            };
            assert_eq!(plant.block, [4, 64, 8]);
            assert_eq!(plant.face, [-1, 0, 0]);
            assert!(
                plant.distance > 2.0 && plant.distance < 3.5,
                "{}",
                plant.distance
            );

            let HostRet::Raycast(Some(solid)) =
                cast(&mut store, 10.0, mod_api::RayFilter::Collidable)
            else {
                panic!("a body's ray reaches the stone");
            };
            assert_eq!(solid.block, [7, 64, 8]);
            assert!((solid.distance - 5.5).abs() < 1e-3, "{}", solid.distance);

            assert_eq!(
                cast(&mut store, 4.0, mod_api::RayFilter::Collidable),
                HostRet::Raycast(None),
                "nothing within max"
            );
            assert!(matches!(
                cast(&mut store, 0.0, mod_api::RayFilter::Collidable),
                HostRet::Error(_)
            ));
            assert!(matches!(
                handle_host_call(
                    &mut store,
                    HostCall::Raycast {
                        from: [1.5, 64.2, 8.5],
                        dir: [0.0, 0.0, 0.0],
                        max: 4.0,
                        filter: mod_api::RayFilter::Selectable,
                    },
                ),
                HostRet::Error(_)
            ));
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
            let unloaded = handle_host_call(
                &mut store,
                HostCall::LightAt {
                    pos: [512, 64, 512],
                },
            );
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
            let mut shape =
                |pos| match handle_host_call(&mut store, HostCall::CollisionShapeAt { pos }) {
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

    /// `FindBlocks` contract: matches come back in the documented scan order
    /// (y, then z, then x ascending), a box touching any unreadable cell
    /// answers `None` whole (never a partial search), and the volume /
    /// inverted-box guards reject before any sim access.
    #[test]
    fn find_blocks_scans_in_order_and_gates_unreadable_boxes() {
        let mut store = ModStoreData::new("alpha", 1);
        let volume_capped = handle_host_call(
            &mut store,
            HostCall::FindBlocks {
                min: [0, 0, 0],
                max: [32, 31, 31],
                blocks: vec![],
            },
        );
        match volume_capped {
            HostRet::Error(e) => assert!(e.contains("volume"), "got '{e}'"),
            other => panic!("over-volume box answered {other:?}"),
        }
        let inverted = handle_host_call(
            &mut store,
            HostCall::FindBlocks {
                min: [0, 5, 0],
                max: [1, 4, 1],
                blocks: vec![],
            },
        );
        assert!(matches!(inverted, HostRet::Error(_)), "inverted box");

        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(world.set_block_world(4, 66, 5, Block::Stone));
        assert!(world.set_block_world(3, 64, 5, Block::Stone));
        assert!(world.set_block_world(8, 64, 2, Block::OakLog));
        with_world_ctx(&mut world, || {
            let stone = vec![mod_api::BlockId(Block::Stone.id())];
            let find = |store: &mut ModStoreData, min, max, blocks| match handle_host_call(
                store,
                HostCall::FindBlocks { min, max, blocks },
            ) {
                HostRet::FoundBlocks(f) => f,
                other => panic!("expected FoundBlocks, got {other:?}"),
            };
            assert_eq!(
                find(&mut store, [0, 60, 0], [15, 70, 15], stone.clone()),
                Some(vec![[3, 64, 5], [4, 66, 5]]),
                "matches in scan order, other species not listed"
            );
            assert_eq!(
                find(&mut store, [10, 60, 10], [20, 70, 20], stone),
                None,
                "a box reaching an unloaded column is unreadable whole"
            );
        });
    }

    /// A non-finite prim is REFUSED, not quietly dropped — because the engine
    /// answers an unchanged resubmission by comparing the submitted prims, and
    /// `NaN != NaN`. Dropping it would leave a machine at rest logging a
    /// replication delta every tick for a box that draws nothing, which is a
    /// pathology with no local symptom at all.
    #[test]
    fn a_non_finite_draw_prim_is_refused_rather_than_dropped() {
        let mut store = ModStoreData::new("alpha", 1);
        let nan = mod_api::DrawPrim::Cuboid {
            min: [0.0, 0.0, 0.0],
            max: [1.0, f32::NAN, 1.0],
            tile: "stone".into(),
            tint: [255, 255, 255],
            emissive: false,
        };
        // Refused BEFORE the sim scope is consulted (there is none here), so a
        // malformed submission cannot depend on where the caller was.
        match handle_host_call(
            &mut store,
            HostCall::SetBlockDraw {
                pos: [0, 0, 0],
                prims: vec![nan],
            },
        ) {
            HostRet::Error(e) => assert!(e.contains("non-finite"), "got '{e}'"),
            other => panic!("a NaN corner answered {other:?}"),
        }
    }

    /// THE TWO DRAW CALLS ANSWER A FOREIGN BLOCK DIFFERENTLY, and that is a
    /// decision rather than an oversight — so it is pinned here, because three
    /// doc comments had already drifted into describing the single call's
    /// error as a `false`.
    ///
    /// A single submission naming a block the mod does not own is a mod bug:
    /// it asked about one cell and got the cell wrong. A BATCHED submission is
    /// the whole kind at once, and one of its machines being broken between
    /// the read and the write is ordinary — erroring there would disable the
    /// pack for losing a race it cannot avoid.
    #[test]
    fn a_batched_draw_answers_per_entry_where_the_single_call_errors() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        // An engine block: loaded, readable, and not this mod's.
        world.set_block_world(4, 64, 4, Block::Stone);
        let foreign = [4, 64, 4];

        with_world_ctx(&mut world, || {
            match handle_host_call(
                &mut store,
                HostCall::SetBlockDraw {
                    pos: foreign,
                    prims: Vec::new(),
                },
            ) {
                HostRet::Error(e) => assert!(e.contains("not owned"), "got '{e}'"),
                other => panic!("a foreign block answered {other:?}, not an error"),
            }
            match handle_host_call(
                &mut store,
                HostCall::SetBlockDraws {
                    sets: vec![(foreign, Vec::new())],
                },
            ) {
                HostRet::Bools(v) => assert_eq!(v, vec![false], "per entry, and the call survives"),
                other => panic!("the batched form answered {other:?}"),
            }
        });
    }

    /// The router's arms are one long `|` chain per handler, so deleting or
    /// moving a variant welds the arm that ended on it onto the next
    /// handler's: every call above that line silently starts going somewhere
    /// else, and the compiler only notices if a whole handler becomes
    /// unreachable. Dispatched through `handle_host_call` — going straight to
    /// `handle_block_call` would prove only that this file has the arm.
    #[test]
    fn block_presentation_calls_route_to_the_block_handler() {
        let mut store = ModStoreData::new("alpha", 1);
        for call in [
            HostCall::SetBlockDraw {
                pos: [0, 0, 0],
                prims: Vec::new(),
            },
            HostCall::SetModelParts {
                pos: [0, 0, 0],
                parts: 0,
                tint: None,
            },
            HostCall::SwapModelBlock {
                pos: [0, 0, 0],
                block: mod_api::BlockId(1),
            },
            HostCall::BlockLocalToWorld {
                pos: [0, 0, 0],
                points: Vec::new(),
            },
        ] {
            let name = format!("{call:?}");
            let ret = handle_host_call(&mut store, call);
            assert!(
                !matches!(&ret, HostRet::Error(e) if e.contains("mis-routed")),
                "{name} did not reach the block handler: {ret:?}"
            );
        }
    }

    /// A footprint-local point must land inside the PLACED footprint at every
    /// facing. This is the whole reason the call exists: an anchor plus a
    /// fixed world offset is right at one facing and puts the point inside or
    /// behind the machine at the other three, and a mod re-deriving the
    /// placement transform is that same rule written a second time.
    #[test]
    fn a_footprint_local_point_follows_the_placed_facing() {
        use petramond_math::facing::Facing;

        let mut store = ModStoreData::new("alpha", 1);
        let base = petramond_math::math::IVec3::new(4, 64, 4);
        let kind = Block::FurnitureWorkbench
            .model_kind()
            .expect("fixture: a model block");
        let size = petramond_world::block_model::def(kind).cells.map(f32::from);
        // Off centre on every axis, so a lost rotation cannot coincide with
        // the right answer.
        let local = [size[0] * 0.8, size[1] * 0.2, size[2] * 0.9];

        let mut seen: Vec<[f32; 3]> = Vec::new();
        for facing in [Facing::North, Facing::East, Facing::South, Facing::West] {
            let mut world = World::new(1, 4);
            world.clear_world();
            for (cx, cz) in [(0, 0), (-1, 0), (0, -1), (-1, -1)] {
                world.insert_empty_column_for_test(ChunkPos::new(cx, cz));
            }
            assert!(
                world.place_model_block_facing(base, Block::FurnitureWorkbench, facing),
                "fixture: the workbench places facing {facing:?}"
            );
            let (_, _, cells) = world.model_group(base).expect("fixture: a placed group");
            with_world_ctx(&mut world, || {
                let got = match handle_host_call(
                    &mut store,
                    HostCall::BlockLocalToWorld {
                        pos: [base.x, base.y, base.z],
                        points: vec![local],
                    },
                ) {
                    HostRet::Points(Some(p)) => p[0],
                    other => panic!("{facing:?}: {other:?}"),
                };
                let inside = cells.iter().any(|c| {
                    (0..3).all(|a| {
                        let lo = [c.x, c.y, c.z][a] as f32;
                        got[a] >= lo && got[a] <= lo + 1.0
                    })
                });
                assert!(inside, "{facing:?}: {got:?} fell outside {cells:?}");
                seen.push(got);
            });
        }
        assert!(
            seen.iter().any(|p| p != &seen[0]),
            "the four facings all answered {:?} — the placement rotation is gone",
            seen[0]
        );
    }

    /// Every other mod read of a cell gates on stream finality; this one has
    /// the extra reason that a machine's FACING is exactly what the saved
    /// overlay about to land carries.
    #[test]
    fn local_to_world_gates_an_unreadable_cell() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        with_world_ctx(&mut world, || {
            assert_eq!(
                handle_host_call(
                    &mut store,
                    HostCall::BlockLocalToWorld {
                        pos: [512, 64, 512],
                        points: vec![[0.5, 0.5, 0.5]],
                    },
                ),
                HostRet::Points(None)
            );
        });
    }
}
