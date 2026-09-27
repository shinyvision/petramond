use mod_api::{BlockCall, HostRet};
use petramond_world::world::raycast;

use petramond_math::math::IVec3;

use super::guards::{
    batch_guard, checked_block, finite3, key_owned_by_namespace, sim_call, sim_query, sim_read,
    stream_final_cell,
};

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
        log::debug!("{call}: block '{name}' at {pos:?} is not mod '{mod_id}''s");
        return Err(HostRet::Bool(false));
    }
    Ok(block)
}

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
        mod_api::DrawPrim::Sprite {
            at,
            scale,
            yaw,
            pitch,
            spin,
            bob,
            ..
        } => at
            .iter()
            .chain([scale, yaw, pitch, spin])
            .chain(bob)
            .all(|v| v.is_finite()),
    }
}

pub(super) fn check_draw_set(what: &str, prims: &[mod_api::DrawPrim]) -> Option<HostRet> {
    const MAX: usize = mod_api::DRAW_PRIMS_MAX;
    if prims.len() > MAX {
        return Some(HostRet::invalid(format!(
            "{what}: {} prims; the cap is {MAX}",
            prims.len()
        )));
    }
    let bad = prims.iter().position(|p| !draw_prim_finite(p))?;
    Some(HostRet::invalid(format!(
        "{what}: prim {bad} has a non-finite component"
    )))
}

pub(super) fn handle_block_call(mod_id: &str, call: BlockCall) -> HostRet {
    match call {
        BlockCall::SetBlockDraws { sets } => {
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
        BlockCall::SetModelPartsMany { sets } => {
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
        BlockCall::SetBlockDraw { pos, prims } => {
            if let Some(err) = check_draw_set("SetBlockDraw", &prims) {
                return err;
            }
            let mod_id = mod_id.to_owned();
            sim_query(
                move |ctx| match owned_block_at(ctx, &mod_id, pos, "SetBlockDraw") {
                    Err(e) => e,
                    Ok(_) => {
                        ctx.world.set_block_draw(IVec3::from(pos), prims.into());
                        HostRet::Bool(true)
                    }
                },
            )
        }
        BlockCall::SetModelParts { pos, parts, tint } => {
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
        BlockCall::BlockLocalToWorld { pos, points } => {
            if let Some(err) = batch_guard("BlockLocalToWorld point", points.len()) {
                return err;
            }
            sim_read(move |ctx| {
                let p = IVec3::from(pos);
                if stream_final_cell(ctx, p).is_err() {
                    return HostRet::Points(None);
                }
                let frame = ctx.world.block_local_frame(p);
                HostRet::Points(Some(
                    points
                        .iter()
                        .map(|&q| {
                            frame
                                .to_world(petramond_math::math::Vec3::from(q))
                                .to_array()
                        })
                        .collect(),
                ))
            })
        }
        BlockCall::SwapBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => {
                let new_name = petramond_world::registry::names()
                    .blocks
                    .name(b.id())
                    .unwrap_or("?");
                if !key_owned_by_namespace(mod_id, new_name) {
                    return HostRet::invalid(format!(
                        "SwapBlock: block '{new_name}' is not owned by mod '{mod_id}'"
                    ));
                }
                let mod_id = mod_id.to_owned();
                sim_query(
                    move |ctx| match owned_block_at(ctx, &mod_id, pos, "SwapBlock") {
                        Err(e) => e,
                        Ok(_) => HostRet::Bool(ctx.world.swap_block(IVec3::from(pos), b)),
                    },
                )
            }
        },
        BlockCall::BiomeAt { pos } => {
            sim_read(move |ctx| HostRet::MaybeByte(ctx.world.data().biome_at_world(pos[0], pos[1])))
        }
        BlockCall::SurfaceYAt { pos } => sim_read(move |ctx| {
            let y = ctx
                .world
                .data()
                .surface_collision_y(pos[0], pos[1])
                .filter(|&y| ctx.world.block_if_stream_final(pos[0], y, pos[1]).is_some());
            HostRet::MaybeI32(y)
        }),
        BlockCall::GetBlock { pos } => sim_read(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Block(
                ctx.world
                    .block_if_stream_final(p.x, p.y, p.z)
                    .map(|b| mod_api::BlockId(b.id())),
            )
        }),
        BlockCall::GetBlocks { positions } => {
            if let Some(err) = batch_guard("GetBlocks position", positions.len()) {
                return err;
            }
            sim_read(|ctx| {
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
        BlockCall::BlockChangesSince { since } => sim_read(|ctx| {
            let (next, cells, lost) = match since {
                Some(seq) => ctx.world.changes_since(seq),
                None => (ctx.world.changes_since(u64::MAX).0, Vec::new(), false),
            };
            HostRet::BlockChanges(mod_api::BlockChanges {
                next,
                lost,
                cells: cells.iter().map(|c| c.to_array()).collect(),
            })
        }),
        BlockCall::Raycast {
            from,
            dir,
            max,
            filter,
        } => match RaycastQuery::new(from, dir, max, filter) {
            Ok(query) => sim_read(move |ctx| query.against(ctx.world.data())),
            Err(refused) => refused,
        },
        BlockCall::FindBlocks { min, max, blocks } => {
            if let Some(err) = batch_guard("FindBlocks block", blocks.len()) {
                return err;
            }
            if min.iter().zip(&max).any(|(lo, hi)| lo > hi) {
                return HostRet::invalid(format!("FindBlocks: inverted box {min:?}..{max:?}"));
            }
            let volume = min
                .iter()
                .zip(&max)
                .map(|(lo, hi)| (hi - lo) as i64 + 1)
                .product::<i64>();
            if volume > super::guards::FIND_BLOCKS_VOLUME_MAX {
                return HostRet::invalid(format!(
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
            sim_read(move |ctx| {
                let mut found = Vec::new();
                let y_lo = min[1].max(petramond_world::chunk::WORLD_MIN_Y);
                let y_hi = max[1].min(petramond_world::chunk::WORLD_MAX_Y - 1);
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
        BlockCall::SetBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => sim_query(|ctx| {
                let p = IVec3::from(pos);
                HostRet::Bool(ctx.world.set_block_world(p.x, p.y, p.z, b))
            }),
        },
        BlockCall::SetBlocks { blocks } => {
            if let Some(err) = batch_guard("SetBlocks write", blocks.len()) {
                return err;
            }
            sim_query(|ctx| {
                let mut set = 0u64;
                for &(pos, block) in &blocks {
                    let Ok(b) = checked_block(block) else {
                        return HostRet::invalid(format!(
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
        BlockCall::ScheduleTick { pos, delay } => {
            sim_call(|ctx| ctx.world.schedule_tick(pos.into(), delay))
        }
        BlockCall::IsLoaded { pos } => sim_read(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bool(ctx.world.section_stream_final_at(p.x, p.y, p.z))
        }),
        BlockCall::LightAt { pos } => sim_read(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Light(ctx.world.block_if_stream_final(p.x, p.y, p.z).map(|_| {
                mod_api::LightData {
                    combined: ctx.world.data().combined_light6_at_world(p.x, p.y, p.z),
                    sky: ctx.world.data().skylight6_at_world(p.x, p.y, p.z),
                    block: ctx.world.data().blocklight6_at_world(p.x, p.y, p.z),
                    block_rgb: ctx.world.data().blocklight6_rgb_at_world(p.x, p.y, p.z),
                }
            }))
        }),
        BlockCall::LightAtMany { positions } => {
            if let Some(err) = batch_guard("LightAtMany position", positions.len()) {
                return err;
            }
            sim_read(|ctx| {
                HostRet::Lights(
                    positions
                        .into_iter()
                        .map(|pos| {
                            let p = IVec3::from(pos);
                            ctx.world.block_if_stream_final(p.x, p.y, p.z).map(|_| {
                                mod_api::LightData {
                                    combined: ctx
                                        .world
                                        .data()
                                        .combined_light6_at_world(p.x, p.y, p.z),
                                    sky: ctx.world.data().skylight6_at_world(p.x, p.y, p.z),
                                    block: ctx.world.data().blocklight6_at_world(p.x, p.y, p.z),
                                    block_rgb: ctx
                                        .world
                                        .data()
                                        .blocklight6_rgb_at_world(p.x, p.y, p.z),
                                }
                            })
                        })
                        .collect(),
                )
            })
        }
        BlockCall::CollisionShapeAt { pos } => sim_read(|ctx| {
            let p = IVec3::from(pos);
            HostRet::CollisionShape(ctx.world.block_if_stream_final(p.x, p.y, p.z).map(|_| {
                match ctx.world.data().collision_shape_class(p.x, p.y, p.z) {
                    crate::world::CollisionShapeClass::Empty => mod_api::CollisionShape::Empty,
                    crate::world::CollisionShapeClass::Partial => mod_api::CollisionShape::Partial,
                    crate::world::CollisionShapeClass::Full => mod_api::CollisionShape::Full,
                }
            }))
        }),
    }
}

pub(in crate::modding) struct RaycastQuery {
    from: petramond_math::world_pos::WorldPos,
    dir: glam::Vec3,
    max: f32,
    filter: crate::player::RayFilter,
}

impl RaycastQuery {
    pub(in crate::modding) fn new(
        from: [f64; 3],
        dir: [f32; 3],
        max: f32,
        filter: mod_api::RayFilter,
    ) -> Result<Self, HostRet> {
        let from = super::guards::finite_pos(from, "Raycast.from")?;
        let dir = match finite3(dir, "Raycast.dir")? {
            v if v.length_squared() > f32::EPSILON => v.normalize(),
            _ => return Err(HostRet::invalid("Raycast: zero direction".into())),
        };
        if !max.is_finite() || max <= 0.0 || max > mod_api::RAYCAST_MAX_DISTANCE {
            return Err(HostRet::invalid(format!(
                "Raycast: max must be finite and in (0, {}]",
                mod_api::RAYCAST_MAX_DISTANCE
            )));
        }
        let filter = match filter {
            mod_api::RayFilter::Selectable => crate::player::RayFilter::Selectable,
            mod_api::RayFilter::Collidable => crate::player::RayFilter::Collidable,
        };
        Ok(Self {
            from,
            dir,
            max,
            filter,
        })
    }

    pub(in crate::modding) fn against(&self, world: &petramond_world::world::WorldData) -> HostRet {
        HostRet::Raycast(
            raycast::filtered(self.from, self.dir, self.max, self.filter, world).map(
                |(hit, distance)| mod_api::RaycastHitData {
                    block: hit.block.to_array(),
                    face: hit.normal.to_array(),
                    distance,
                },
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{calls, CollisionShape, HostCall, HostRet};

    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, RosterRefs, SimCtx};
    use crate::modding::host::guards::SIM_BATCH_MAX;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::world::ServerWorld;
    use petramond_world::block::Block;
    use petramond_world::chunk::ChunkPos;

    fn with_world_ctx(world: &mut ServerWorld, f: impl FnOnce()) {
        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, f);
    }

    #[test]
    fn batched_calls_reject_oversized_batches() {
        let mut store = ModStoreData::new("alpha", 1);
        for (name, call) in [
            (
                "GetBlocks",
                HostCall::from(calls::GetBlocks {
                    positions: vec![[0, 0, 0]; SIM_BATCH_MAX + 1],
                }),
            ),
            (
                "SetBlocks",
                HostCall::from(calls::SetBlocks {
                    blocks: vec![([0, 0, 0], mod_api::BlockId(0)); SIM_BATCH_MAX + 1],
                }),
            ),
            (
                "ContainerGetMany",
                HostCall::from(calls::ContainerGetMany {
                    addresses: vec![[0, 0, 0].into(); SIM_BATCH_MAX + 1],
                }),
            ),
            (
                "ContainerSet",
                HostCall::from(calls::ContainerSet {
                    at: [0, 0, 0].into(),
                    slots: vec![(0, None); SIM_BATCH_MAX + 1],
                }),
            ),
            (
                "ItemNames",
                HostCall::from(calls::ItemNames {
                    items: vec![mod_api::ItemId(0); SIM_BATCH_MAX + 1],
                }),
            ),
        ] {
            match handle_host_call(&mut store, call) {
                HostRet::Err(e) => assert!(
                    e.detail.contains("exceeds"),
                    "{name}: expected the cap error, got '{e}'"
                ),
                other => panic!("{name}: over-cap batch answered {other:?}"),
            }
        }
        let got = handle_host_call(
            &mut store,
            HostCall::from(calls::ItemNames {
                items: vec![mod_api::ItemId(0); SIM_BATCH_MAX],
            }),
        );
        assert!(matches!(got, HostRet::Names(v) if v.len() == SIM_BATCH_MAX));
        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        with_world_ctx(&mut world, || {
            let got = handle_host_call(
                &mut store,
                HostCall::from(calls::GetBlocks {
                    positions: vec![[8, 64, 8]; SIM_BATCH_MAX],
                }),
            );
            assert!(matches!(got, HostRet::Blocks(v) if v.len() == SIM_BATCH_MAX));
        });
    }

    #[test]
    fn raycast_filters_stop_on_what_they_say_and_report_the_distance() {
        use petramond_world::block::Block;
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.set_block_world(4, 64, 8, Block::Poppy);
        world.set_block_world(7, 64, 8, Block::Stone);
        let cast = |store: &mut ModStoreData, max: f32, filter: mod_api::RayFilter| {
            handle_host_call(
                store,
                HostCall::from(calls::Raycast {
                    from: [1.5, 64.2, 8.5],
                    dir: [2.0, 0.0, 0.0],
                    max,
                    filter,
                }),
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
                HostRet::Err(_)
            ));
            assert!(matches!(
                handle_host_call(
                    &mut store,
                    HostCall::from(calls::Raycast {
                        from: [1.5, 64.2, 8.5],
                        dir: [0.0, 0.0, 0.0],
                        max: 4.0,
                        filter: mod_api::RayFilter::Selectable,
                    }),
                ),
                HostRet::Err(_)
            ));
        });
    }

    #[test]
    fn light_at_answers_none_for_unloaded_cells() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        with_world_ctx(&mut world, || {
            let loaded = handle_host_call(
                &mut store,
                HostCall::from(calls::LightAt { pos: [8, 64, 8] }),
            );
            assert!(
                matches!(loaded, HostRet::Light(Some(_))),
                "loaded cell must answer light, got {loaded:?}"
            );
            let unloaded = handle_host_call(
                &mut store,
                HostCall::from(calls::LightAt {
                    pos: [512, 64, 512],
                }),
            );
            assert_eq!(unloaded, HostRet::Light(None));
        });
    }

    #[test]
    fn collision_shape_classifies_geometry_and_gates_unloaded() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(world.set_block_world(8, 63, 8, Block::Stone));
        assert!(world.set_block_world(8, 64, 8, Block::OakStairs));
        assert!(world.set_block_world(8, 65, 8, Block::Water));
        with_world_ctx(&mut world, || {
            let mut shape = |pos| match handle_host_call(
                &mut store,
                HostCall::from(calls::CollisionShapeAt { pos }),
            ) {
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

    #[test]
    fn find_blocks_scans_in_order_and_gates_unreadable_boxes() {
        let mut store = ModStoreData::new("alpha", 1);
        let volume_capped = handle_host_call(
            &mut store,
            HostCall::from(calls::FindBlocks {
                min: [0, 0, 0],
                max: [32, 31, 31],
                blocks: vec![],
            }),
        );
        match volume_capped {
            HostRet::Err(e) => assert!(e.detail.contains("volume"), "got '{e}'"),
            other => panic!("over-volume box answered {other:?}"),
        }
        let inverted = handle_host_call(
            &mut store,
            HostCall::from(calls::FindBlocks {
                min: [0, 5, 0],
                max: [1, 4, 1],
                blocks: vec![],
            }),
        );
        assert!(matches!(inverted, HostRet::Err(_)), "inverted box");

        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(world.set_block_world(4, 66, 5, Block::Stone));
        assert!(world.set_block_world(3, 64, 5, Block::Stone));
        assert!(world.set_block_world(8, 64, 2, Block::OakLog));
        with_world_ctx(&mut world, || {
            let stone = vec![mod_api::BlockId(Block::Stone.id())];
            let find = |store: &mut ModStoreData, min, max, blocks| match handle_host_call(
                store,
                HostCall::from(calls::FindBlocks { min, max, blocks }),
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
        match handle_host_call(
            &mut store,
            HostCall::from(calls::SetBlockDraw {
                pos: [0, 0, 0],
                prims: vec![nan],
            }),
        ) {
            HostRet::Err(e) => assert!(e.detail.contains("non-finite"), "got '{e}'"),
            other => panic!("a NaN corner answered {other:?}"),
        }
    }

    #[test]
    fn a_draw_on_someone_elses_block_answers_false_and_the_mod_lives() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.set_block_world(4, 64, 4, Block::Stone);
        let foreign = [4, 64, 4];

        with_world_ctx(&mut world, || {
            match handle_host_call(
                &mut store,
                HostCall::from(calls::SetBlockDraw {
                    pos: foreign,
                    prims: Vec::new(),
                }),
            ) {
                HostRet::Bool(false) => {}
                other => panic!("a foreign block answered {other:?}"),
            }
            match handle_host_call(
                &mut store,
                HostCall::from(calls::SetBlockDraws {
                    sets: vec![(foreign, Vec::new())],
                }),
            ) {
                HostRet::Bools(v) => assert_eq!(v, vec![false], "per entry, and the call survives"),
                other => panic!("the batched form answered {other:?}"),
            }
        });
    }

    #[test]
    fn block_presentation_calls_route_to_the_block_handler() {
        let mut store = ModStoreData::new("alpha", 1);
        for call in [
            HostCall::from(calls::SetBlockDraw {
                pos: [0, 0, 0],
                prims: Vec::new(),
            }),
            HostCall::from(calls::SetModelParts {
                pos: [0, 0, 0],
                parts: 0,
                tint: None,
            }),
            HostCall::from(calls::SwapBlock {
                pos: [0, 0, 0],
                block: mod_api::BlockId(1),
            }),
            HostCall::from(calls::BlockLocalToWorld {
                pos: [0, 0, 0],
                points: Vec::new(),
            }),
        ] {
            let name = format!("{call:?}");
            let ret = handle_host_call(&mut store, call);
            assert!(
                !matches!(&ret, HostRet::Err(e) if e.detail.contains("mis-routed")),
                "{name} did not reach the block handler: {ret:?}"
            );
        }
    }

    #[test]
    fn a_footprint_local_point_follows_the_placed_facing() {
        use petramond_math::facing::Facing;

        let mut store = ModStoreData::new("alpha", 1);
        let base = petramond_math::math::IVec3::new(4, 64, 4);
        let kind = Block::FurnitureWorkbench
            .model_kind()
            .expect("fixture: a model block");
        let size = petramond_world::block_model::def(kind).cells.map(f32::from);
        let local = [size[0] * 0.8, size[1] * 0.2, size[2] * 0.9];

        let mut seen: Vec<[f64; 3]> = Vec::new();
        for facing in [Facing::North, Facing::East, Facing::South, Facing::West] {
            let mut world = ServerWorld::new(1, 4);
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
                    HostCall::from(calls::BlockLocalToWorld {
                        pos: [base.x, base.y, base.z],
                        points: vec![local],
                    }),
                ) {
                    HostRet::Points(Some(p)) => p[0],
                    other => panic!("{facing:?}: {other:?}"),
                };
                let inside = cells.iter().any(|c| {
                    (0..3).all(|a| {
                        let lo = [c.x, c.y, c.z][a] as f32;
                        got[a] >= f64::from(lo) && got[a] <= f64::from(lo + 1.0)
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

    #[test]
    fn local_to_world_gates_an_unreadable_cell() {
        let mut store = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 4);
        world.clear_world();
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        with_world_ctx(&mut world, || {
            assert_eq!(
                handle_host_call(
                    &mut store,
                    HostCall::from(calls::BlockLocalToWorld {
                        pos: [512, 64, 512],
                        points: vec![[0.5, 0.5, 0.5]],
                    }),
                ),
                HostRet::Points(None)
            );
        });
    }
}
