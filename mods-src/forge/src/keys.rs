//! Every pack id this mod names, declared once and checked against the
//! shipped pack data by the test below.
//!
//! Instance data a stack carries (the augment record) and cell/world KV keys
//! are not pack declarations; they stay with the code that owns them. So do
//! recipe CLASSES (`forge:cast_plate`): a class is a value recipe rows share,
//! not a row of its own.

mod_sdk::pack_keys! {
    /// The forging furnace: its placeable row, item and recipe, and the lit
    /// and pouring variants.
    pub FORGING_FURNACE: Block = "forge:forging_furnace";
    pub FORGING_FURNACE_LIT: Block = "forge:forging_furnace_lit";
    pub FORGING_FURNACE_POUR: Block = "forge:forging_furnace_pour";
    pub FORGING_FURNACE_ITEM: Item = "forge:forging_furnace";
    pub FORGING_FURNACE_RECIPE: Recipe = "forge:forging_furnace";
    /// The anvil's placeable row and its recipe.
    pub ANVIL: Block = "forge:anvil";
    pub ANVIL_RECIPE: Recipe = "forge:anvil";
    pub POTTERY_TABLE_RECIPE: Recipe = "forge:pottery_table";
    /// The class cast when the basin holds no mould: metal poured into bare
    /// stone sets as a plate.
    pub CAST_PLATE_CLASS: RecipeClass = "forge:cast_plate";
    /// The ore vein this pack generates, and the engine stone it replaces.
    pub PETRAMOND_ORE: Block = "forge:petramond_ore";
    pub STONE: Block = "petramond:stone";

    /// Clay: the deposit block, and the lump whose first pickup reveals the
    /// pottery table; a first diamond reveals the anvil.
    pub CLAY_BLOCK: Block = "petramond:clay";
    pub CLAY_ITEM: Item = "petramond:clay";
    pub DIAMOND: Item = "petramond:diamond";
    /// Engine ground a clay deposit may replace.
    pub GRASS: Block = "petramond:grass";
    pub DIRT: Block = "petramond:dirt";
    pub COARSE_DIRT: Block = "petramond:coarse_dirt";
    pub PODZOL: Block = "petramond:podzol";
    pub SAND: Block = "petramond:sand";
    pub RED_SAND: Block = "petramond:red_sand";
    pub GRAVEL: Block = "petramond:gravel";

    /// Anything the player pulls out of a fresh ore vein.
    pub RAW_ORE_TAG: Tag = "petramond:raw_ore";

    /// Row-data keys: casting (moulds and metals), furnace storage and feed,
    /// the fittings table, and an augmentable tool's socket row.
    pub MOULD_DATA: Data = "forge:mould";
    pub METAL_DATA: Data = "forge:metal";
    pub STORAGE_DATA: Data = "forge:storage";
    pub FEED_DATA: Data = "forge:feed";
    pub UPGRADES_DATA: Data = "forge:upgrades";
    pub AUGMENT_SLOTS_DATA: Data = "forge:augment_slots";
    /// The row-data key an augment MATERIAL carries: its list of fits (tool
    /// kind, edge tier, multipliers, cost, overlay art, optional behaviour
    /// grant).
    pub AUGMENT_DATA: Data = "forge:augment";
    /// The data key that marks an item as a SOCKET material (the petramond
    /// gem): consumed at the anvil to carve a locked socket open or raise a
    /// mount level. Membership is the whole vocabulary — the value is not
    /// read.
    pub SOCKET_KEY_DATA: Data = "forge:socket_key";
    /// Row-data key marking an item as an innately gentle miner. Gold's
    /// policy declares the behaviour; the anvil refuses fitting a `gentle`
    /// augment to a tool that already has it (no gold-on-gold).
    pub NONDESTRUCTIVE_DATA: Data = "forge:nondestructive";

    /// The furnace's four audible moments, and the anvil's two.
    pub SOUND_FIRE: Sound = "forge:fire_catch";
    pub SOUND_LEVER: Sound = "forge:lever";
    pub SOUND_LAND: Sound = "forge:pour_land";
    pub SOUND_CAST: Sound = "forge:cast_free";
    pub SOUND_AUGMENT: Sound = "forge:augment_fit";
    pub SOUND_SOCKET: Sound = "forge:augment_unlock";

    /// The container documents.
    pub FURNACE_GUI: GuiKind = "forge:forging_furnace";
    pub FITTINGS_GUI: GuiKind = "forge:forging_furnace_fittings";
    pub ANVIL_GUI: GuiKind = "forge:anvil";

    /// The furnace panel's pour lever: clicking it is the whole pour control;
    /// its DRAWN frame is [`LEVER_FRAME`], published from the machine state
    /// rather than its own latch, so the graphic can only ever show what the
    /// machine is actually doing.
    pub WIDGET_LEVER: Widget(FURNACE_GUI) = "lever";
    /// The furnace panel's button to the fittings page.
    pub WIDGET_FITTINGS: Widget(FURNACE_GUI) = "fittings";
    /// The fittings page's way back to the furnace panel.
    pub WIDGET_BACK: Widget(FITTINGS_GUI) = "back";
    /// The anvil panel's Augment button.
    pub WIDGET_AUGMENT: Widget(ANVIL_GUI) = "augment";

    /// The furnace panel's gauges, crucible tint and tip, and lever chrome.
    pub BURN01: GuiState = "forge:burn01";
    pub MELT01: GuiState = "forge:melt01";
    pub CRUCIBLE01: GuiState = "forge:crucible01";
    pub CRUCIBLE_COLOR: GuiState = "forge:crucible_color";
    pub CRUCIBLE_TIP: GuiState = "forge:crucible_tip";
    pub LEVER_FRAME: GuiState = "forge:lever_frame";
    pub LEVER_ENABLED: GuiState = "forge:lever_enabled";
}

/// The anvil panel's state keys.
pub mod anvil {
    mod_sdk::pack_keys! {
        /// The enlarged tool stage, the stage hint, the result preview lines
        /// and the Augment button's enabled state.
        pub TOOL_VIEW: GuiState = "forge:tool_view";
        pub ANVIL_HINT: GuiState = "forge:anvil_hint";
        pub PREVIEW_SPEED: GuiState = "forge:preview_speed";
        pub PREVIEW_DAMAGE: GuiState = "forge:preview_damage";
        pub PREVIEW_KNOCKBACK: GuiState = "forge:preview_knockback";
        pub PREVIEW_GENTLE: GuiState = "forge:preview_gentle";
        pub CAN_AUGMENT: GuiState = "forge:can_augment";
        /// Per socket cell: chrome frame, ghost icon and admission mask.
        pub SOCK0_ST: GuiState = "forge:sock0_st";
        pub SOCK0_GHOST: GuiState = "forge:sock0_ghost";
        pub SOCK0_ACC: GuiState = "forge:sock0_acc";
        pub SOCK1_ST: GuiState = "forge:sock1_st";
        pub SOCK1_GHOST: GuiState = "forge:sock1_ghost";
        pub SOCK1_ACC: GuiState = "forge:sock1_acc";
        pub SOCK2_ST: GuiState = "forge:sock2_st";
        pub SOCK2_GHOST: GuiState = "forge:sock2_ghost";
        pub SOCK2_ACC: GuiState = "forge:sock2_acc";
        pub SOCK3_ST: GuiState = "forge:sock3_st";
        pub SOCK3_GHOST: GuiState = "forge:sock3_ghost";
        pub SOCK3_ACC: GuiState = "forge:sock3_acc";
    }

    /// One socket cell's state keys.
    pub struct SocketKeys {
        /// Chrome frame index (nothing / lock / no-socket cover).
        pub st: &'static str,
        /// The installed augment's dimmed material icon.
        pub ghost: &'static str,
        /// The admission mask over the cell's authored filters.
        pub acc: &'static str,
    }

    /// The socket cells' keys, by socket index.
    pub const SOCKET_CELLS: [SocketKeys; 4] = [
        SocketKeys {
            st: SOCK0_ST,
            ghost: SOCK0_GHOST,
            acc: SOCK0_ACC,
        },
        SocketKeys {
            st: SOCK1_ST,
            ghost: SOCK1_GHOST,
            acc: SOCK1_ACC,
        },
        SocketKeys {
            st: SOCK2_ST,
            ghost: SOCK2_GHOST,
            acc: SOCK2_ACC,
        },
        SocketKeys {
            st: SOCK3_ST,
            ghost: SOCK3_GHOST,
            acc: SOCK3_ACC,
        },
    ];
}

/// The fittings page: per fitting, its buy button and the state its tile
/// binds.
pub mod fittings {
    mod_sdk::pack_keys! {
        /// The quench fitting: its buy button, then every state key its tile binds.
        pub QUENCH: Widget(super::FITTINGS_GUI) = "fit_quench";
        pub QUENCH_NAME: GuiState = "forge:fit_quench_name";
        pub QUENCH_IMAGE: GuiState = "forge:fit_quench_image";
        pub QUENCH_INACTIVE_IMAGE: GuiState = "forge:fit_quench_inactive_image";
        pub QUENCH_UNBOUGHT: GuiState = "forge:fit_quench_unbought";
        pub QUENCH_INFO: GuiState = "forge:fit_quench_info";
        pub QUENCH_COST: GuiState = "forge:fit_quench_cost";
        pub QUENCH_STATUS: GuiState = "forge:fit_quench_status";
        pub QUENCH_PALETTE: GuiState = "forge:fit_quench_palette";
        pub QUENCH_ON: GuiState = "forge:fit_quench_on";
        pub QUENCH_BOUGHT: GuiState = "forge:fit_quench_bought";
        /// The counterweight fitting: its buy button, then every state key its tile binds.
        pub COUNTERWEIGHT: Widget(super::FITTINGS_GUI) = "fit_counterweight";
        pub COUNTERWEIGHT_NAME: GuiState = "forge:fit_counterweight_name";
        pub COUNTERWEIGHT_IMAGE: GuiState = "forge:fit_counterweight_image";
        pub COUNTERWEIGHT_INACTIVE_IMAGE: GuiState = "forge:fit_counterweight_inactive_image";
        pub COUNTERWEIGHT_UNBOUGHT: GuiState = "forge:fit_counterweight_unbought";
        pub COUNTERWEIGHT_INFO: GuiState = "forge:fit_counterweight_info";
        pub COUNTERWEIGHT_COST: GuiState = "forge:fit_counterweight_cost";
        pub COUNTERWEIGHT_STATUS: GuiState = "forge:fit_counterweight_status";
        pub COUNTERWEIGHT_PALETTE: GuiState = "forge:fit_counterweight_palette";
        pub COUNTERWEIGHT_ON: GuiState = "forge:fit_counterweight_on";
        pub COUNTERWEIGHT_BOUGHT: GuiState = "forge:fit_counterweight_bought";
        /// The chute fitting: its buy button, then every state key its tile binds.
        pub CHUTE: Widget(super::FITTINGS_GUI) = "fit_chute";
        pub CHUTE_NAME: GuiState = "forge:fit_chute_name";
        pub CHUTE_IMAGE: GuiState = "forge:fit_chute_image";
        pub CHUTE_INACTIVE_IMAGE: GuiState = "forge:fit_chute_inactive_image";
        pub CHUTE_UNBOUGHT: GuiState = "forge:fit_chute_unbought";
        pub CHUTE_INFO: GuiState = "forge:fit_chute_info";
        pub CHUTE_COST: GuiState = "forge:fit_chute_cost";
        pub CHUTE_STATUS: GuiState = "forge:fit_chute_status";
        pub CHUTE_PALETTE: GuiState = "forge:fit_chute_palette";
        pub CHUTE_ON: GuiState = "forge:fit_chute_on";
        pub CHUTE_BOUGHT: GuiState = "forge:fit_chute_bought";
        /// The stoker fitting: its buy button, then every state key its tile binds.
        pub STOKER: Widget(super::FITTINGS_GUI) = "fit_stoker";
        pub STOKER_NAME: GuiState = "forge:fit_stoker_name";
        pub STOKER_IMAGE: GuiState = "forge:fit_stoker_image";
        pub STOKER_INACTIVE_IMAGE: GuiState = "forge:fit_stoker_inactive_image";
        pub STOKER_UNBOUGHT: GuiState = "forge:fit_stoker_unbought";
        pub STOKER_INFO: GuiState = "forge:fit_stoker_info";
        pub STOKER_COST: GuiState = "forge:fit_stoker_cost";
        pub STOKER_STATUS: GuiState = "forge:fit_stoker_status";
        pub STOKER_PALETTE: GuiState = "forge:fit_stoker_palette";
        pub STOKER_ON: GuiState = "forge:fit_stoker_on";
        pub STOKER_BOUGHT: GuiState = "forge:fit_stoker_bought";
    }

    /// One fitting's buy button and tile state keys.
    pub struct FittingKeys {
        pub widget: &'static str,
        pub name: &'static str,
        pub image: &'static str,
        pub inactive_image: &'static str,
        pub unbought: &'static str,
        pub info: &'static str,
        pub cost: &'static str,
        pub status: &'static str,
        pub palette: &'static str,
        pub on: &'static str,
        pub bought: &'static str,
    }

    /// Every fitting's keys, in `furnace::fittings::KINDS` order.
    pub const TABLE: [FittingKeys; 4] = [
        FittingKeys {
            widget: QUENCH,
            name: QUENCH_NAME,
            image: QUENCH_IMAGE,
            inactive_image: QUENCH_INACTIVE_IMAGE,
            unbought: QUENCH_UNBOUGHT,
            info: QUENCH_INFO,
            cost: QUENCH_COST,
            status: QUENCH_STATUS,
            palette: QUENCH_PALETTE,
            on: QUENCH_ON,
            bought: QUENCH_BOUGHT,
        },
        FittingKeys {
            widget: COUNTERWEIGHT,
            name: COUNTERWEIGHT_NAME,
            image: COUNTERWEIGHT_IMAGE,
            inactive_image: COUNTERWEIGHT_INACTIVE_IMAGE,
            unbought: COUNTERWEIGHT_UNBOUGHT,
            info: COUNTERWEIGHT_INFO,
            cost: COUNTERWEIGHT_COST,
            status: COUNTERWEIGHT_STATUS,
            palette: COUNTERWEIGHT_PALETTE,
            on: COUNTERWEIGHT_ON,
            bought: COUNTERWEIGHT_BOUGHT,
        },
        FittingKeys {
            widget: CHUTE,
            name: CHUTE_NAME,
            image: CHUTE_IMAGE,
            inactive_image: CHUTE_INACTIVE_IMAGE,
            unbought: CHUTE_UNBOUGHT,
            info: CHUTE_INFO,
            cost: CHUTE_COST,
            status: CHUTE_STATUS,
            palette: CHUTE_PALETTE,
            on: CHUTE_ON,
            bought: CHUTE_BOUGHT,
        },
        FittingKeys {
            widget: STOKER,
            name: STOKER_NAME,
            image: STOKER_IMAGE,
            inactive_image: STOKER_INACTIVE_IMAGE,
            unbought: STOKER_UNBOUGHT,
            info: STOKER_INFO,
            cost: STOKER_COST,
            status: STOKER_STATUS,
            palette: STOKER_PALETTE,
            on: STOKER_ON,
            bought: STOKER_BOUGHT,
        },
    ];
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[
            super::PACK_KEYS,
            super::anvil::PACK_KEYS,
            super::fittings::PACK_KEYS,
        ]);
    }
}
