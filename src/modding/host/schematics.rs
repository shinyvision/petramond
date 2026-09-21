//! Schematic calls: reading world-held designs as construction records,
//! asking a player's client to choose or position one, and anchoring ghosts.

use mod_api::{
    BlockRecord, HostCall, HostRet, SchematicCellsData, SchematicInfoData, SchematicLookup,
};
use petramond_math::math::IVec3;

use super::guards::{key_owned_by_namespace, sim_mutating_query, sim_query};
use crate::events::DeferredAction;
use crate::schematic::store::Lookup;

pub(super) fn handle_schematic_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SchematicInfo { asset } => sim_query(|ctx| {
            HostRet::Schematic(match ctx.world.schematics_mut().store.lookup(&asset) {
                Lookup::Missing => SchematicLookup::Missing,
                Lookup::Loading => SchematicLookup::Loading,
                Lookup::Failed(reason) => SchematicLookup::Failed { reason },
                Lookup::Ready(asset) => SchematicLookup::Ready(SchematicInfoData {
                    title: asset.schematic.name.clone(),
                    size: asset.schematic.size,
                    cells: asset.schematic.cell_count() as u64,
                    sections: asset.schematic.sections.len() as u32,
                }),
            })
        }),
        HostCall::SchematicCells {
            asset,
            section,
            turns,
        } => sim_query(|ctx| {
            let Lookup::Ready(asset) = ctx.world.schematics_mut().store.lookup(&asset) else {
                return HostRet::SchematicCells(None);
            };
            HostRet::SchematicCells(
                asset
                    .schematic
                    .construction_section(section as usize, turns)
                    .map(|(cells, palette)| SchematicCellsData {
                        cells,
                        palette: palette
                            .into_iter()
                            .map(|data| BlockRecord {
                                block: data.block,
                                state: data.state,
                                refs: data.state_ids.into_iter().collect(),
                                data: data.kv.into_iter().collect(),
                            })
                            .collect(),
                    }),
            )
        }),
        HostCall::SchematicChoose { player, tag } => {
            if !key_owned_by_namespace(mod_id, &tag) {
                return namespace_error("SchematicChoose tag", mod_id, &tag);
            }
            sim_mutating_query(|ctx| {
                let player = crate::player::PlayerId(player.0);
                if ctx.session_index(player).is_none() {
                    return HostRet::Bool(false);
                }
                ctx.queue
                    .push_action(DeferredAction::SchematicChoose { player, tag });
                HostRet::Bool(true)
            })
        }
        HostCall::SchematicPosition {
            player,
            tag,
            asset,
            origin,
            turns,
        } => {
            if !key_owned_by_namespace(mod_id, &tag) {
                return namespace_error("SchematicPosition tag", mod_id, &tag);
            }
            sim_mutating_query(|ctx| {
                let player = crate::player::PlayerId(player.0);
                if ctx.session_index(player).is_none()
                    || !ctx.world.schematics().store.contains(&asset)
                {
                    return HostRet::Bool(false);
                }
                ctx.queue.push_action(DeferredAction::SchematicPosition {
                    player,
                    tag,
                    asset,
                    origin: origin.map(IVec3::from_array),
                    turns: turns % 4,
                });
                HostRet::Bool(true)
            })
        }
        HostCall::SchematicGhostSet { key, ghost } => {
            if !key_owned_by_namespace(mod_id, &key) {
                return namespace_error("SchematicGhostSet key", mod_id, &key);
            }
            sim_mutating_query(|ctx| {
                let ghosts = &mut ctx.world.schematics_mut().ghosts;
                match ghost {
                    None => {
                        ghosts.remove(&key);
                        HostRet::Bool(true)
                    }
                    Some(ghost) => {
                        if !ctx.world.schematics().store.contains(&ghost.asset) {
                            return HostRet::Bool(false);
                        }
                        ctx.world.schematics_mut().ghosts.insert(
                            key,
                            crate::world::schematic::Ghost {
                                placement: crate::schematic::share::GhostPlacement {
                                    digest: ghost.asset,
                                    origin: ghost.origin,
                                    turns: ghost.turns % 4,
                                    yields_to_positioning: ghost.yields_to_positioning,
                                },
                                viewers: ghost
                                    .viewers
                                    .iter()
                                    .map(|p| crate::player::PlayerId(p.0))
                                    .collect(),
                            },
                        );
                        HostRet::Bool(true)
                    }
                }
            })
        }
        other => HostRet::Error(format!(
            "non-schematic call {other:?} mis-routed to handle_schematic_call (host bug)"
        )),
    }
}

fn namespace_error(what: &str, mod_id: &str, key: &str) -> HostRet {
    HostRet::Error(format!(
        "{what} '{key}' must use this mod's own namespace ('{mod_id}:name')"
    ))
}
