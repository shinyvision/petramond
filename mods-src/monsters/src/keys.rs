//! Every pack id this mod names, declared once and checked against the
//! shipped pack data by the test below.

mod_sdk::pack_keys! {
    /// This pack's two hostile species.
    pub ZOMBIE: Mob = "monsters:zombie";
    pub HUSHJAW: Mob = "monsters:hushjaw";
    /// The engine condition sunlight ignites.
    pub BURNING: Condition = "petramond:burning";
    /// Block tag marking a surface no hostile spawns ON. Any pack lists it on
    /// any `blocks.json` row and that block is spawn-proof everywhere in the
    /// world, with no code change here and none in the engine — which knows
    /// neither this tag nor any block that carries it. See `SpawnProof`.
    pub SPAWN_PROOF_TAG: Tag = "monsters:spawn_proof";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
