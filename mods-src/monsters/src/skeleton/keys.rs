mod_sdk::pack_keys! {
    pub SKELETON: Mob = "monsters:skeleton";
    pub POST_NODE: AiNode = "monsters:skeleton_post";
    pub LOADOUTS_DATA: Data = "monsters:loadouts";
    pub UNARMED_DATA: Data = "monsters:unarmed";
    pub WIELD_DATA: Data = "monsters:wield";
    pub DROPS: Loot = "monsters:skeleton_drops";
    pub BLOCK_SOUND: Sound = "petramond:wood_punch";
}

pub const PROJECTILE_DATA: &str = "petramond:projectile";

pub const LOADOUT_TAG: &str = "monsters:loadout";
pub const POST_TAG: &str = "monsters:post";
pub const ROLE_TAG: &str = "monsters:role";
pub const FACING_TAG: &str = "monsters:facing";
pub const RETURNING_TAG: &str = "monsters:returning";

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
