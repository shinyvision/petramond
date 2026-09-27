use mod_api::{PlayerId, SchematicCellsData, SchematicGhostData, SchematicId, SchematicLookup};

use crate::__rt::host_fn;

host_fn! {
    pub fn schematic_info(asset: SchematicId) -> SchematicLookup
        => SchematicInfo { asset } => Schematic
}

host_fn! {
    pub fn schematic_cells(asset: SchematicId, section: u32, turns: u8) -> Option<SchematicCellsData>
        => SchematicCells { asset, section, turns } => SchematicCells
}

host_fn! {
    pub fn schematic_choose(player: PlayerId, tag: &str) -> bool
        => SchematicChoose { player, tag: tag.into() } => Bool
}

host_fn! {
    pub fn schematic_position(player: PlayerId, tag: &str, asset: SchematicId, origin: Option<[i32; 3]>, turns: u8) -> bool
        => SchematicPosition { player, tag: tag.into(), asset, origin, turns } => Bool
}

host_fn! {
    pub fn schematic_ghost_set(key: &str, ghost: Option<SchematicGhostData>) -> bool
        => SchematicGhostSet { key: key.into(), ghost } => Bool
}
