mod_sdk::pack_keys! {
    pub ZOMBIE: Mob = "monsters:zombie";
    pub HUSHJAW: Mob = "monsters:hushjaw";
    pub BURNING: Condition = "petramond:burning";
    pub SPAWN_PROOF_TAG: Tag = "monsters:spawn_proof";
    pub CAMP_HUT_LOOT: Loot = "monsters:camp_hut_chest";
    pub CAMP_ARENA_LOOT: Loot = "monsters:camp_arena_chest";
}

/// Cell data the camp generator writes on a spawn post's floor cell (`[role, yaw]`) and the
/// skeleton garrison reads back.
pub const POST_MARKER: &str = "monsters:camp_post";

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
