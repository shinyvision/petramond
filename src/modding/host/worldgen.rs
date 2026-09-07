//! Worldgen calls: the init-window-gated gen hook registrations plus the
//! UNDERGROUND-BIOME vocabulary. (Block/item name resolution lives in the
//! `registry` domain.)
//!
//! The underground-biome query lives here rather than in `registry` because it
//! needs the store's world seed: it is a pure function of (seed, position)
//! reading the same cave field the carver reads, so it is legal on the
//! DETACHED worldgen instances — no `SimCtx`, no loaded section.

use mod_api::{HostCall, HostRet};

use super::guards::batch_guard;
use super::{ModStoreData, Registration};

/// Worldgen-hook calls (the gen registrations) and the underground-biome
/// vocabulary.
pub(super) fn handle_worldgen_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    match call {
        HostCall::ResolveUndergroundBiome { key } => {
            HostRet::MaybeByte(petramond_worldgen::data::underground::id_by_name(&key))
        }
        HostCall::UndergroundBiomeAt { positions } => {
            match batch_guard("UndergroundBiomeAt position", positions.len()) {
                Some(err) => err,
                None => HostRet::UndergroundBiomes(petramond_worldgen::underground_biomes_at(
                    data.world_seed(),
                    &positions,
                )),
            }
        }
        HostCall::UndergroundBiomesInBox { lo, hi } => HostRet::UndergroundBiomes(
            petramond_worldgen::underground_biomes_in_box(data.world_seed(), lo, hi),
        ),
        HostCall::TerrainSolidAt { positions } => {
            match batch_guard("TerrainSolidAt position", positions.len()) {
                Some(err) => err,
                None => HostRet::TerrainSolid(petramond_worldgen::terrain_solid_at(
                    data.world_seed(),
                    &positions,
                )),
            }
        }
        HostCall::SurfaceBiomeAt { columns } => {
            match batch_guard("SurfaceBiomeAt column", columns.len()) {
                Some(err) => err,
                None => HostRet::SurfaceBiomes(petramond_worldgen::surface_biome_at(
                    data.world_seed(),
                    &columns,
                )),
            }
        }
        HostCall::RegisterWorldgenFeature {
            feature_id,
            stage,
            filter,
        } => {
            if !filter.is_valid() {
                return data.refuse_registration(format!(
                    "worldgen feature {feature_id}: write bounds are inverted ({filter:?})"
                ));
            }
            if stage == mod_api::WorldgenStage::Climate {
                return data.refuse_registration(
                    "worldgen features cannot attach after the climate stage (it is \
                     column-level, before any blocks exist); use Terrain or later",
                );
            }
            data.register(Registration::WorldgenFeature {
                stage,
                feature_id,
                filter,
            })
        }
        HostCall::RegisterStageReplacement { stage, callback_id } => {
            data.register(Registration::StageReplacement { stage, callback_id })
        }
        HostCall::RegisterGenerator { callback_id } => {
            data.register(Registration::Generator { callback_id })
        }
        other => HostRet::Error(format!(
            "non-worldgen call {other:?} mis-routed to handle_worldgen_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests;
