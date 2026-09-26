//! The pack's registry names resolved to session ids, once, at init.
//!
//! Numeric ids are session-scoped and never persisted, so every other module
//! works against this struct rather than re-resolving names per dispatch (a
//! host call inside a per-cell worldgen loop is the one thing that reliably
//! trips the mod watchdog).

use mod_sdk::*;

use crate::fluids::Fluids;
use crate::keys;

/// One mushroom species: a colour the whole cavern palette is built from.
/// Adding a species is ONE row here plus its pack JSON — never a match arm.
pub struct Species {
    pub cap: BlockId,
    pub sporeshroom: BlockId,
    pub flower: BlockId,
    /// The luminous vine segment. Lives here rather than in a flat list beside
    /// `Content` so a curtain blooms in the colour of the stand it hangs in.
    pub glow_vine: BlockId,
}

pub struct Content {
    pub stem: BlockId,
    pub vine: BlockId,
    /// A still water SOURCE. Resolved from the ENGINE's row — a pack does not
    /// get to invent its own fluid, and the containment proof in `cascade.rs` is
    /// written against the behaviour that row declares.
    pub water: BlockId,
    /// Nothing of this pack is placed in or on a fluid: it is neither ground
    /// to stand on nor room to grow into.
    pub fluids: Fluids,
    /// Pond bed, weir lip and shore. Solid and opaque, which is load-bearing
    /// twice over: it is what walls the water in, and what holds up flora
    /// dressed on the shore.
    pub silt: BlockId,
    /// Plain air. A cascade CUTS its gorge, so it needs to write the absence
    /// of a block as well as the presence of one.
    pub air: BlockId,
    pub species: Vec<Species>,
}

impl Content {
    /// Is a block the SECTION SNAPSHOT holds a fluid? The positional terrain
    /// queries answer this outside the section; inside it the snapshot is the
    /// truth, and it carries ids, not spaces.
    pub fn is_fluid(&self, block: BlockId) -> bool {
        self.fluids.contains(block)
    }
}

/// One species' four rows, by registry name.
struct SpeciesRows {
    cap: &'static str,
    sporeshroom: &'static str,
    flower: &'static str,
    glow_vine: &'static str,
}

/// The species palette. Adding a species is one row here plus its pack JSON.
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
    /// Resolve the cavern palette. A species with a missing row is dropped
    /// (the resolve logs which row) and the caverns grow in the colours that
    /// remain; only a missing structure row, or losing every species, turns
    /// the feature off.
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
