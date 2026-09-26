//! Every pack id this mod names, declared once and checked against the
//! shipped pack data by the test below.

mod_sdk::pack_keys! {
    /// The placeable oven row, and the lit variant swapped in while it burns.
    pub OVEN_BLOCK: Block = "kitchen:oven";
    pub OVEN_LIT_BLOCK: Block = "kitchen:oven_lit";
    /// The miller row, and the variant showing flour in its output.
    pub MILLER_BLOCK: Block = "kitchen:miller";
    pub MILLER_FULL_BLOCK: Block = "kitchen:miller_full";

    /// The dish served in a vessel, and the vessel it hands back.
    pub RABBIT_STEW: Item = "kitchen:rabbit_stew";
    pub WOODEN_BOWL: Item = "kitchen:wooden_bowl";

    /// The container documents the machines open.
    pub OVEN_GUI: GuiKind = "kitchen:oven";
    pub MILLER_GUI: GuiKind = "kitchen:miller";

    /// The oven's cook-progress arrow and fuel flame gauges.
    pub COOK01: GuiState = "kitchen:cook01";
    pub BURN01: GuiState = "kitchen:burn01";
    /// The miller's grind-progress gauge.
    pub MILL01: GuiState = "kitchen:mill01";

    /// The recipe classes the machines cook and grind by — rows any pack
    /// (farming's bread, flour) joins by naming the class.
    pub COOKING_CLASS: RecipeClass = "kitchen:cooking";
    pub MILLING_CLASS: RecipeClass = "kitchen:milling";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
