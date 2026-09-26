//! Every pack id this mod names, declared once and checked against the
//! shipped JSON by the test below: registry rows, row data keys, tags, the
//! brain node, and each GUI document's kind, bind keys and widget ids.
//!
//! Keys that are not pack declarations — world-KV prefixes, rng streams,
//! instance data a blueprint carries at runtime — stay beside the code that
//! owns them.

mod_sdk::pack_keys! {
    pub TABLE_BLOCK: Block = "builder:schematic_table";
    pub TABLE_ITEM: Item = "builder:schematic_table";
    pub BLUEPRINT: Item = "builder:blueprint";
    pub RAW_COPPER: Item = "petramond:raw_copper";
    /// The row a cell the design keeps empty is recorded as.
    pub AIR: Block = "petramond:air";
    pub GOLEM: Mob = "builder:mason_golem";
    pub COPPER_BLOCK_RECIPE: Recipe = "builder:copper_block";
    pub EARTH_BURST: Emitter = "builder:earth_burst";
    /// The golem's steering node, named on its `mobs.json` brain.
    pub WORKER_NODE: AiNode = "builder:worker";
    /// Row data a block opts into scaffolding with: any pack's plain, sturdy
    /// block joins by patching its row.
    pub SCAFFOLDING_DATA: Data = "builder:scaffolding";
    /// Row data marking a container a table draws supplies from.
    pub SUPPLY_DATA: Data = "builder:supply";
    /// Blocks laid last: they decay unless what they live beside already
    /// stands.
    pub LEAVES_TAG: Tag = "leaves";
    pub FRAGILE_TAG: Tag = "fragile";
}

/// The schematic table's document.
pub mod table {
    mod_sdk::pack_keys! {
        pub KIND: GuiKind = "builder:schematic_table";
        pub TITLE: GuiState = "builder:title";
        pub STATUS: GuiState = "builder:status";
        pub STATUS_PALETTE: GuiState = "builder:status_palette";
        pub CAN_CHOOSE: GuiState = "builder:can_choose";
        pub CAN_POSITION: GuiState = "builder:can_position";
        pub HAS_DESIGN: GuiState = "builder:has_design";
        pub CAN_GHOST: GuiState = "builder:can_ghost";
        pub GHOST_FRAME: GuiState = "builder:ghost_frame";
        pub SHOW_START: GuiState = "builder:show_start";
        pub CAN_START: GuiState = "builder:can_start";
        pub SHOW_PAUSE: GuiState = "builder:show_pause";
        pub SHOW_RESUME: GuiState = "builder:show_resume";
        pub CAN_RESUME: GuiState = "builder:can_resume";
        pub SHOW_CANCEL: GuiState = "builder:show_cancel";
        pub MATERIALS: Widget("builder:schematic_table") = "materials";
        pub CHOOSE: Widget("builder:schematic_table") = "choose";
        pub POSITION: Widget("builder:schematic_table") = "position";
        pub START: Widget("builder:schematic_table") = "start";
        pub PAUSE: Widget("builder:schematic_table") = "pause";
        pub RESUME: Widget("builder:schematic_table") = "resume";
        pub CANCEL: Widget("builder:schematic_table") = "cancel";
        pub GHOST: Widget("builder:schematic_table") = "ghost";
    }
}

/// The materials page's document: the bill list and each row's fields.
pub mod materials {
    mod_sdk::pack_keys! {
        pub KIND: GuiKind = "builder:materials";
        pub BACK: Widget("builder:materials") = "back";
        pub BILL: GuiState = "builder:bill";
        pub ROW_ITEM: GuiState = "item";
        pub ROW_TEXT: GuiState = "text";
        pub ROW_DONE: GuiState = "done";
        pub ROW_NOTE: GuiState = "note";
        pub ROW_NOTE_PALETTE: GuiState = "note_palette";
    }
}

/// The golem's own panel.
pub mod golem {
    mod_sdk::pack_keys! {
        pub KIND: GuiKind = "builder:golem";
        pub STATUS: GuiState = "builder:golem_status";
        pub STATUS_PALETTE: GuiState = "builder:golem_status_palette";
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[
            super::PACK_KEYS,
            super::table::PACK_KEYS,
            super::materials::PACK_KEYS,
            super::golem::PACK_KEYS,
        ]);
    }
}
