mod_sdk::pack_keys! {
    pub(crate) MUSHROOM_CAVERN: UndergroundBiome = "exploration:mushroom_cavern";
    pub(crate) DRIPSTONE_CAVES: UndergroundBiome = "exploration:dripstone_caves";
    pub(crate) SPORE_DRIFT: Emitter = "exploration:spore_drift";

    pub(crate) MUSHROOM_STEM: Block = "exploration:mushroom_stem";
    pub(crate) HANGING_VINE: Block = "exploration:hanging_vine";
    pub(crate) CAVE_SILT: Block = "exploration:cave_silt";

    pub(crate) GLOWCAP_PINK: Block = "exploration:glowcap_pink";
    pub(crate) GLOWCAP_BLUE: Block = "exploration:glowcap_blue";
    pub(crate) GLOWCAP_MAGENTA: Block = "exploration:glowcap_magenta";
    pub(crate) GLOWCAP_PURPLE: Block = "exploration:glowcap_purple";
    pub(crate) SPORESHROOM_PINK: Block = "exploration:sporeshroom_pink";
    pub(crate) SPORESHROOM_BLUE: Block = "exploration:sporeshroom_blue";
    pub(crate) SPORESHROOM_MAGENTA: Block = "exploration:sporeshroom_magenta";
    pub(crate) SPORESHROOM_PURPLE: Block = "exploration:sporeshroom_purple";
    pub(crate) CAVE_FLOWER_PINK: Block = "exploration:cave_flower_pink";
    pub(crate) CAVE_FLOWER_BLUE: Block = "exploration:cave_flower_blue";
    pub(crate) CAVE_FLOWER_MAGENTA: Block = "exploration:cave_flower_magenta";
    pub(crate) CAVE_FLOWER_PURPLE: Block = "exploration:cave_flower_purple";
    pub(crate) GLOW_VINE_PINK: Block = "exploration:glow_vine_pink";
    pub(crate) GLOW_VINE_BLUE: Block = "exploration:glow_vine_blue";
    pub(crate) GLOW_VINE_MAGENTA: Block = "exploration:glow_vine_magenta";
    pub(crate) GLOW_VINE_PURPLE: Block = "exploration:glow_vine_purple";

    pub(crate) DRIPSTONE_BLOCK: Block = "exploration:dripstone_block";
    pub(crate) STALACTITE: Block = "exploration:stalactite";
    pub(crate) STALACTITE_WET: Block = "exploration:stalactite_wet";
    pub(crate) STALAGMITE: Block = "exploration:stalagmite";
    pub(crate) POINTED_DRIPSTONE_HOOK: Behavior = "exploration:pointed_dripstone";
    pub(crate) POINTED_DRIPSTONE_ITEM: Item = "exploration:pointed_dripstone";
    pub(crate) DRIP_VESSEL: Data = "exploration:drip_vessel";

    pub(crate) WATER: Block = "petramond:water";
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }

    #[test]
    fn the_biome_altitude_gates_match_their_rows() {
        use mod_sdk::json::Value;
        let rows = Value::parse(include_str!("../pack/underground_biomes.json"))
            .expect("underground_biomes.json parses");
        let top = |key: &str| {
            rows.get("underground_biomes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|row| row.get("underground_biome").and_then(Value::as_str) == Some(key))
                .and_then(|row| row.get("y")?.as_array()?.get(1)?.as_i32())
                .unwrap_or_else(|| panic!("{key} declares a y band"))
        };
        assert_eq!(top(super::MUSHROOM_CAVERN), crate::BIOME_TOP_Y);
        assert_eq!(top(super::DRIPSTONE_CAVES), crate::dripstone::BIOME_TOP_Y);
    }
}
