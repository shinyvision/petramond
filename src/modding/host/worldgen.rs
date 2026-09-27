use mod_api::{ErrorCode, HostRet, WorldgenCall};

use super::guards::batch_guard;
use super::{ModStoreData, Registration};

pub(super) fn handle_worldgen_call(data: &mut ModStoreData, call: WorldgenCall) -> HostRet {
    match call {
        WorldgenCall::ResolveUndergroundBiome { key } => {
            HostRet::MaybeByte(petramond_worldgen::data::underground::id_by_name(&key))
        }
        WorldgenCall::UndergroundBiomeAt { positions } => {
            match batch_guard("UndergroundBiomeAt position", positions.len()) {
                Some(err) => err,
                None => HostRet::UndergroundBiomes(petramond_worldgen::underground_biomes_at(
                    data.world_seed(),
                    &positions,
                )),
            }
        }
        WorldgenCall::UndergroundBiomesInBox { lo, hi } => HostRet::UndergroundBiomes(
            petramond_worldgen::underground_biomes_in_box(data.world_seed(), lo, hi),
        ),
        WorldgenCall::TerrainBlocksAt { positions } => {
            match batch_guard("TerrainBlocksAt position", positions.len()) {
                Some(err) => err,
                None => HostRet::BlockList(
                    petramond_worldgen::terrain_blocks_at(data.world_seed(), &positions)
                        .into_iter()
                        .map(mod_api::BlockId)
                        .collect(),
                ),
            }
        }
        WorldgenCall::TerrainSectionAt { section } => HostRet::SectionBlocks(
            petramond_worldgen::terrain_section_at(data.world_seed(), section)
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        WorldgenCall::TerrainHeightsAt { columns } => {
            match batch_guard("TerrainHeightsAt column", columns.len()) {
                Some(err) => err,
                None => HostRet::TerrainHeights(petramond_worldgen::terrain_heights_at(
                    data.world_seed(),
                    &columns,
                )),
            }
        }
        WorldgenCall::TerrainSolidAt { positions } => {
            match batch_guard("TerrainSolidAt position", positions.len()) {
                Some(err) => err,
                None => HostRet::TerrainSolid(petramond_worldgen::terrain_solid_at(
                    data.world_seed(),
                    &positions,
                )),
            }
        }
        WorldgenCall::TerrainSpaceAt { positions } => {
            match batch_guard("TerrainSpaceAt position", positions.len()) {
                Some(err) => err,
                None => HostRet::TerrainSpaces(petramond_worldgen::terrain_space_at(
                    data.world_seed(),
                    &positions,
                )),
            }
        }
        WorldgenCall::SurfaceBiomeAt { columns } => {
            match batch_guard("SurfaceBiomeAt column", columns.len()) {
                Some(err) => err,
                None => HostRet::SurfaceBiomes(petramond_worldgen::surface_biome_at(
                    data.world_seed(),
                    &columns,
                )),
            }
        }
        WorldgenCall::RegisterWorldgenFeature {
            feature_id,
            stage,
            filter,
        } => {
            if !filter.is_valid() {
                return data.refuse_registration(
                    ErrorCode::InvalidArgument,
                    format!(
                        "worldgen feature {feature_id}: write bounds are inverted ({filter:?})"
                    ),
                );
            }
            if stage == mod_api::WorldgenStage::Climate {
                return data.refuse_registration(
                    ErrorCode::InvalidArgument,
                    "worldgen features cannot attach after the climate stage (it is \
                     column-level, before any blocks exist); use Terrain or later"
                        .into(),
                );
            }
            data.register(Registration::WorldgenFeature {
                stage,
                feature_id,
                filter,
            })
        }
        WorldgenCall::RegisterStageReplacement { stage, callback_id } => {
            data.register(Registration::StageReplacement { stage, callback_id })
        }
        WorldgenCall::RegisterGenerator { callback_id } => {
            data.register(Registration::Generator { callback_id })
        }
    }
}

#[cfg(test)]
mod tests;
