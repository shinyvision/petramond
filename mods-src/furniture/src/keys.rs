mod_sdk::pack_keys! {
    pub(crate) CHAIR: Block = "furniture:chair";
    pub(crate) BENCH: Block = "furniture:bench";
    pub(crate) SEATS: Data = "furniture:seats";

    pub(crate) CHAIN_SHAPE: Shape = "furniture:chain";
    pub(crate) CHAIN: Block = "furniture:chain";
    pub(crate) CHAIN_NS: Block = "furniture:chain_ns";
    pub(crate) CHAIN_EW: Block = "furniture:chain_ew";

    pub(crate) LANTERN_SHAPE: Shape = "furniture:lantern";
    pub(crate) LANTERN: Block = "furniture:lantern";
    pub(crate) LANTERN_HANGING: Block = "furniture:lantern_hanging";
    pub(crate) LANTERN_WALL_NORTH: Block = "furniture:lantern_wall_north";
    pub(crate) LANTERN_WALL_SOUTH: Block = "furniture:lantern_wall_south";
    pub(crate) LANTERN_WALL_WEST: Block = "furniture:lantern_wall_west";
    pub(crate) LANTERN_WALL_EAST: Block = "furniture:lantern_wall_east";

    pub(crate) CAULDRON_SHAPE: Shape = "furniture:cauldron";
    pub(crate) CAULDRON: Block = "furniture:cauldron";
    pub(crate) CAULDRON_WATER: Block = "furniture:cauldron_water";
    pub(crate) CAULDRON_DYE: Block = "furniture:cauldron_dye";

    pub(crate) DYEABLE: Data = "furniture:dyeable";
    pub(crate) PIGMENT: Data = "furniture:pigment";
    pub(crate) PAINTABLE: Data = "furniture:paintable";

    pub(crate) TABLE_GUI: GuiKind = "furniture:painting_table";
    pub(crate) CANVAS: Widget("furniture:painting_table") = "canvas";
    pub(crate) WHEEL: Widget("furniture:painting_table") = "wheel";
    pub(crate) HUE: Widget("furniture:painting_table") = "hue";
    pub(crate) VALUE: Widget("furniture:painting_table") = "value";
    pub(crate) SWATCHES: Widget("furniture:painting_table") = "swatches";
    pub(crate) PENCIL: Widget("furniture:painting_table") = "pencil";
    pub(crate) FILL: Widget("furniture:painting_table") = "fill";
    pub(crate) UNDO: Widget("furniture:painting_table") = "undo";
    pub(crate) CLEAR: Widget("furniture:painting_table") = "clear";
    pub(crate) FLAG_SLOT: Widget("furniture:painting_table") = "flag";
    pub(crate) INVENTORY: Widget("furniture:painting_table") = "inventory";
    pub(crate) HOTBAR: Widget("furniture:painting_table") = "hotbar";
    pub(crate) CANVAS_IMAGE: GuiState = "furniture:canvas_image";
    pub(crate) WHEEL_IMAGE: GuiState = "furniture:wheel_image";
    pub(crate) HUE_IMAGE: GuiState = "furniture:hue_image";
    pub(crate) VALUE_IMAGE: GuiState = "furniture:value_image";
    pub(crate) SWATCHES_IMAGE: GuiState = "furniture:swatches_image";
    pub(crate) EMPTY: GuiState = "furniture:empty";
    pub(crate) PENCIL_ON: GuiState = "furniture:pencil_on";
    pub(crate) FILL_ON: GuiState = "furniture:fill_on";
    pub(crate) CAN_UNDO: GuiState = "furniture:can_undo";

    #[cfg(test)]
    pub(crate) CABINET_GUI: GuiKind = "furniture:cabinet";
    #[cfg(test)]
    pub(crate) COUNTER_CABINET_GUI: GuiKind = "furniture:counter_cabinet";
}

/// The client's request to paint the flag in the table it has open; the data is the paint.
pub(crate) const PAINT_EVENT: &str = "furniture:paint";

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS, mod_sdk::WATER_BUCKET_KEYS]);
    }
}
