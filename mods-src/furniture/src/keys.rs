//! Every pack id this mod names, declared once and checked against the
//! shipped JSON by the test below (see [`mod_sdk::pack_keys!`]).

mod_sdk::pack_keys! {
    /// Sit-able pieces (`seats::PIECES`).
    pub(crate) CHAIR: Block = "furniture:chair";
    pub(crate) BENCH: Block = "furniture:bench";

    /// The chain family: one custom shape over three axis rows.
    pub(crate) CHAIN_SHAPE: Shape = "furniture:chain";
    pub(crate) CHAIN: Block = "furniture:chain";
    pub(crate) CHAIN_NS: Block = "furniture:chain_ns";
    pub(crate) CHAIN_EW: Block = "furniture:chain_ew";

    /// The lantern family: one custom shape over the standing, hanging and
    /// four wall rows (wall rows in `lanterns::WALL_SIDES` order).
    pub(crate) LANTERN_SHAPE: Shape = "furniture:lantern";
    pub(crate) LANTERN: Block = "furniture:lantern";
    pub(crate) LANTERN_HANGING: Block = "furniture:lantern_hanging";
    pub(crate) LANTERN_WALL_NORTH: Block = "furniture:lantern_wall_north";
    pub(crate) LANTERN_WALL_SOUTH: Block = "furniture:lantern_wall_south";
    pub(crate) LANTERN_WALL_WEST: Block = "furniture:lantern_wall_west";
    pub(crate) LANTERN_WALL_EAST: Block = "furniture:lantern_wall_east";

    /// The cauldron family: the pot shape and its fill-state rows.
    pub(crate) CAULDRON_SHAPE: Shape = "furniture:cauldron";
    pub(crate) CAULDRON: Block = "furniture:cauldron";
    pub(crate) CAULDRON_WATER: Block = "furniture:cauldron_water";
    pub(crate) CAULDRON_DYE: Block = "furniture:cauldron_dye";

    /// Item-data interop keys (see `cauldron`).
    pub(crate) DYEABLE: Data = "furniture:dyeable";
    pub(crate) PIGMENT: Data = "furniture:pigment";

    /// Storage cabinets' container documents — named only by the
    /// `cabinet_documents` test (the engine opens them from the block rows).
    #[cfg(test)]
    pub(crate) CABINET_GUI: GuiKind = "furniture:cabinet";
    #[cfg(test)]
    pub(crate) COUNTER_CABINET_GUI: GuiKind = "furniture:counter_cabinet";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS, mod_sdk::WATER_BUCKET_KEYS]);
    }
}
