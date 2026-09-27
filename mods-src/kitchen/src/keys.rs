mod_sdk::pack_keys! {
    pub OVEN_BLOCK: Block = "kitchen:oven";
    pub OVEN_LIT_BLOCK: Block = "kitchen:oven_lit";
    pub MILLER_BLOCK: Block = "kitchen:miller";
    pub MILLER_FULL_BLOCK: Block = "kitchen:miller_full";

    pub RABBIT_STEW: Item = "kitchen:rabbit_stew";
    pub WOODEN_BOWL: Item = "kitchen:wooden_bowl";

    pub OVEN_GUI: GuiKind = "kitchen:oven";
    pub MILLER_GUI: GuiKind = "kitchen:miller";

    pub COOK01: GuiState = "kitchen:cook01";
    pub BURN01: GuiState = "kitchen:burn01";
    pub MILL01: GuiState = "kitchen:mill01";

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
