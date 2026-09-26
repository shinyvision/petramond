//! Every pack id this mod names, declared once and checked against the
//! shipped pack data by the test below.

mod_sdk::pack_keys! {
    /// The shield's registry name (`items.json` row).
    pub SHIELD_ITEM: Item = "combat:shield";
    /// The one-shot played when the guard absorbs a hit (`sounds.json` row).
    pub BLOCK_SOUND: Sound = "combat:shield_block";
    /// The item-row data key naming a bow and its draw:
    /// `{"draw_ticks", "strain_ticks", "draw_speed_scale", "launch_speed":
    /// [weakest, fullest], "pull"?: [frame item names, weakest first]}`.
    pub BOW_KEY: Data = "combat:bow";
    /// The item-row data key naming an arrow and its damage by arrival
    /// speed: `{"damage_weak": [min, max], "damage_full": [min, max],
    /// "speed_weak", "speed_full"}` — the ranges dealt arriving at the two
    /// speeds (m/s), linear between, clamped outside.
    pub ARROW_KEY: Data = "combat:arrow";
    /// Species row data marking a mob that takes every PACED hit: the engine
    /// i-frame is stripped from a hit whose attacker's swings this pack
    /// already paces, because the clock does the i-frame's job — one hit per
    /// arc — and the window would only swallow chained combos. Species
    /// policy as DATA: this pack's `integrations/monsters/mobs.json` patches
    /// it onto the monster rows (present only while that pack is), and any
    /// pack may patch it onto its own. Hits from unpaced hands (bare fists,
    /// another pack's weapon) keep the engine window.
    pub PACED_COMBO_DATA: Data = "combat:paced_combo";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }
}
