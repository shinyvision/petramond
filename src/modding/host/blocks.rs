//! Block calls: stream-final reads, full-edit-path writes, scheduled
//! ticks, light queries, spawn support, and the model-group swap.

use mod_api::{HostCall, HostRet};

use crate::mathh::IVec3;

use super::guards::{checked_block, key_owned_by_namespace, sim_call, sim_query};

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
                    // Stream-final read: while a saved overlay is in flight
                    // the cell shows the generated base — a foreign block —
                    // which must read as "unloaded", not as a namespace
                    // violation.
                    let Some(old) = ctx.world.block_if_stream_final(p.x, p.y, p.z) else {
                        return HostRet::Bool(false);
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
        HostCall::GetBlocks { positions } => sim_query(|ctx| {
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
        }),
        HostCall::SetBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => sim_query(|ctx| {
                let p = IVec3::from(pos);
                HostRet::Bool(ctx.world.set_block_world(p.x, p.y, p.z, b))
            }),
        },
        HostCall::SetBlocks { blocks } => sim_query(|ctx| {
            let mut set = 0u64;
            for &(pos, block) in &blocks {
                let Ok(b) = checked_block(block) else {
                    return HostRet::Error(format!("SetBlocks: unregistered block id {}", block.0));
                };
                let p = IVec3::from(pos);
                if ctx.world.set_block_world(p.x, p.y, p.z, b) {
                    set += 1;
                }
            }
            HostRet::U64(set)
        }),
        HostCall::ScheduleTick { pos, delay } => {
            sim_call(|ctx| ctx.world.schedule_tick(pos.into(), delay))
        }
        HostCall::IsLoaded { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bool(ctx.world.section_stream_final_at(p.x, p.y, p.z))
        }),
        HostCall::LightAt { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Light {
                combined: ctx.world.combined_light6_at_world(p.x, p.y, p.z),
                sky: ctx.world.skylight6_at_world(p.x, p.y, p.z),
                block: ctx.world.blocklight6_at_world(p.x, p.y, p.z),
            }
        }),
        HostCall::BlockIsFullSpawnSupport { pos } => sim_query(|ctx| {
            let p = IVec3::from(pos);
            HostRet::Bool(ctx.world.block_is_full_spawn_support(p.x, p.y, p.z))
        }),
        other => HostRet::Error(format!(
            "non-block call {other:?} mis-routed to handle_block_call (host bug)"
        )),
    }
}
