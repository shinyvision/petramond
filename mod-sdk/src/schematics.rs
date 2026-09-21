//! Schematic calls: world-held designs read as construction records, a
//! player's client asked to choose or position one, and anchored ghosts.

use mod_api::{PlayerId, SchematicCellsData, SchematicGhostData, SchematicId, SchematicLookup};

use crate::__rt::host_fn;

host_fn! {
    /// A world-held schematic's facts; the first ask starts decoding it in
    /// the background ([`SchematicLookup::Loading`]). Server only.
    pub fn schematic_info(asset: SchematicId) -> SchematicLookup
        => SchematicInfo { asset } => Schematic
}

host_fn! {
    /// Stored section `section` of a decoded schematic turned `turns` quarter
    /// turns clockwise, as construction records. `None` = not decoded yet or
    /// no such section. Server only.
    pub fn schematic_cells(asset: SchematicId, section: u32, turns: u8) -> Option<SchematicCellsData>
        => SchematicCells { asset, section, turns } => SchematicCells
}

host_fn! {
    /// Ask `player`'s client to choose a schematic for `tag` (this mod's
    /// namespace); the choice arrives as `schematic_chosen`. `false` = no
    /// such player.
    pub fn schematic_choose(player: PlayerId, tag: &str) -> bool
        => SchematicChoose { player, tag: tag.into() } => Bool
}

host_fn! {
    /// Ask `player`'s client to position `asset` for `tag` (this mod's
    /// namespace), starting at `origin` when given; anchoring arrives as
    /// `schematic_positioned`. `false` = no such player or asset.
    pub fn schematic_position(player: PlayerId, tag: &str, asset: SchematicId, origin: Option<[i32; 3]>, turns: u8) -> bool
        => SchematicPosition { player, tag: tag.into(), asset, origin, turns } => Bool
}

host_fn! {
    /// Anchor, move or remove (`None`) the ghost `key` (this mod's
    /// namespace). Presentation only: re-set it after a restart. `false` =
    /// no such asset.
    pub fn schematic_ghost_set(key: &str, ghost: Option<SchematicGhostData>) -> bool
        => SchematicGhostSet { key: key.into(), ghost } => Bool
}
