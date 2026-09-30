mod_sdk::pack_keys! {
    pub SHIELD_ITEM: Item = "combat:shield";
    pub BLOCK_SOUND: Sound = "combat:shield_block";
    pub BOW_KEY: Data = "combat:bow";
    pub ARROW_KEY: Data = "combat:arrow";
    pub BOOMERANG_KEY: Data = "combat:boomerang";
    pub PACED_COMBO_DATA: Data = "combat:paced_combo";
    pub FAMILY_DATA: Data = "combat:family";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
