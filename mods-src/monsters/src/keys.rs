mod_sdk::pack_keys! {
    pub ZOMBIE: Mob = "monsters:zombie";
    pub HUSHJAW: Mob = "monsters:hushjaw";
    pub BURNING: Condition = "petramond:burning";
    pub SPAWN_PROOF_TAG: Tag = "monsters:spawn_proof";
    pub CAMP_HUT_LOOT: Loot = "monsters:camp_hut_chest";
    pub CAMP_ARENA_LOOT: Loot = "monsters:camp_arena_chest";
    pub SKULL_FLAG: Block = "monsters:skull_flag";
    pub FLAGPOLE: Block = "flags:flagpole";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
