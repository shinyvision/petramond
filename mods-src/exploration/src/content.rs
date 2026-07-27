//! The pack's registry names resolved to session ids, once, at init.
//!
//! Numeric ids are session-scoped and never persisted, so every other module
//! works against this struct rather than re-resolving names per dispatch (a
//! host call inside a per-cell worldgen loop is the one thing that reliably
//! trips the mod watchdog).

use mod_sdk::*;

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
    /// Pond bed, weir lip and shore. Solid and opaque, which is load-bearing
    /// twice over: it is what walls the water in, and what holds up flora
    /// dressed on the shore.
    pub silt: BlockId,
    /// Plain air. A cascade CUTS its gorge, so it needs to write the absence
    /// of a block as well as the presence of one.
    pub air: BlockId,
    pub species: Vec<Species>,
}

const SPECIES_NAMES: [&str; 4] = ["pink", "blue", "magenta", "purple"];

impl Content {
    pub fn resolve() -> Option<Content> {
        let mut species = Vec::with_capacity(SPECIES_NAMES.len());
        for name in SPECIES_NAMES {
            species.push(Species {
                cap: resolve_block_logged(&format!("exploration:glowcap_{name}"))?,
                sporeshroom: resolve_block_logged(&format!("exploration:sporeshroom_{name}"))?,
                flower: resolve_block_logged(&format!("exploration:cave_flower_{name}"))?,
                glow_vine: resolve_block_logged(&format!("exploration:glow_vine_{name}"))?,
            });
        }
        Some(Content {
            stem: resolve_block_logged("exploration:mushroom_stem")?,
            vine: resolve_block_logged("exploration:hanging_vine")?,
            water: resolve_block_logged("petramond:water")?,
            silt: resolve_block_logged("exploration:cave_silt")?,
            air: BlockId(0),
            species,
        })
    }
}
