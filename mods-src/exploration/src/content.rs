use mod_sdk::*;

use crate::fluids::Fluids;
use crate::keys;

pub struct Species {
    pub cap: BlockId,
    pub sporeshroom: BlockId,
    pub flower: BlockId,
    pub glow_vine: BlockId,
}

pub struct Content {
    pub stem: BlockId,
    pub vine: BlockId,
    pub water: BlockId,
    pub fluids: Fluids,
    pub silt: BlockId,
    pub air: BlockId,
    pub species: Vec<Species>,
}

impl Content {
    pub fn is_fluid(&self, block: BlockId) -> bool {
        self.fluids.contains(block)
    }
}

struct SpeciesRows {
    cap: &'static str,
    sporeshroom: &'static str,
    flower: &'static str,
    glow_vine: &'static str,
}

const SPECIES_ROWS: [SpeciesRows; 4] = [
    SpeciesRows {
        cap: keys::GLOWCAP_PINK,
        sporeshroom: keys::SPORESHROOM_PINK,
        flower: keys::CAVE_FLOWER_PINK,
        glow_vine: keys::GLOW_VINE_PINK,
    },
    SpeciesRows {
        cap: keys::GLOWCAP_BLUE,
        sporeshroom: keys::SPORESHROOM_BLUE,
        flower: keys::CAVE_FLOWER_BLUE,
        glow_vine: keys::GLOW_VINE_BLUE,
    },
    SpeciesRows {
        cap: keys::GLOWCAP_MAGENTA,
        sporeshroom: keys::SPORESHROOM_MAGENTA,
        flower: keys::CAVE_FLOWER_MAGENTA,
        glow_vine: keys::GLOW_VINE_MAGENTA,
    },
    SpeciesRows {
        cap: keys::GLOWCAP_PURPLE,
        sporeshroom: keys::SPORESHROOM_PURPLE,
        flower: keys::CAVE_FLOWER_PURPLE,
        glow_vine: keys::GLOW_VINE_PURPLE,
    },
];

impl SpeciesRows {
    fn resolve(&self) -> Option<Species> {
        Some(Species {
            cap: resolve_block_logged(self.cap)?,
            sporeshroom: resolve_block_logged(self.sporeshroom)?,
            flower: resolve_block_logged(self.flower)?,
            glow_vine: resolve_block_logged(self.glow_vine)?,
        })
    }
}

impl Content {
    pub fn resolve(fluids: Fluids) -> Option<Content> {
        let species: Vec<Species> = SPECIES_ROWS
            .iter()
            .filter_map(SpeciesRows::resolve)
            .collect();
        if species.is_empty() {
            return None;
        }
        Some(Content {
            stem: resolve_block_logged(keys::MUSHROOM_STEM)?,
            vine: resolve_block_logged(keys::HANGING_VINE)?,
            water: resolve_block_logged(keys::WATER)?,
            fluids,
            silt: resolve_block_logged(keys::CAVE_SILT)?,
            air: BlockId::AIR,
            species,
        })
    }
}
