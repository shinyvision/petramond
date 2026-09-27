mod_sdk::pack_keys! {
    pub ZOMBIE: Mob = "monsters:zombie";
    pub HUSHJAW: Mob = "monsters:hushjaw";
    pub BURNING: Condition = "petramond:burning";
    pub SPAWN_PROOF_TAG: Tag = "monsters:spawn_proof";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
