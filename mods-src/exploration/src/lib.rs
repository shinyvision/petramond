//! Exploration: rare, cathedral-scale mushroom caverns deep underground.
//!
//! The mod owns POLICY (where its biome lives, how big its rooms are, what
//! grows in them). The engine owns PRIMITIVES: a data-driven underground-biome
//! registry, a positional query for which biome a point is in, coloured block
//! light, and an additive room term the cave carver sums into its own cavern
//! threshold. Nothing in the engine knows what a mushroom is.
//!
//! The cathedral is DATA — a `chamber` clause on this pack's
//! `underground_biomes.json` row — precisely so the engine's carvers extend
//! into it. A room the pack STAMPED after the carve was a boolean union
//! against finished geometry: a tunnel reaching its wall ended at a flat face
//! instead of opening into the room.

use mod_sdk::*;

mod cascade;
mod cavern;
mod content;
mod shroom;
mod spores;

use content::Content;

/// Feature id echoed back to [`Mod::gen_feature`].
const GEN_CAVERN: u32 = 1;

/// The underground biome this pack registers in `underground_biomes.json`.
/// Placement asks the engine for the biome id at a position and compares — the
/// mod never re-implements the engine's selection noise.
pub const BIOME_KEY: &str = "exploration:mushroom_cavern";

/// Top of the depth band that row declares (`"y": [-64, -16]`). Nothing this
/// pack places can be rooted above it, so worldgen derives its altitude gate
/// from here — retuning the band moves this one value with the JSON.
pub const BIOME_TOP_Y: i32 = -16;

#[derive(Default)]
struct Exploration {
    content: Option<Content>,
    spores: spores::Spores,
}

impl Mod for Exploration {
    fn init(&mut self) {
        // The client side runs presentation only — the spore haze — and needs
        // none of the worldgen content, so it is resolved before the content
        // gate and survives a pack whose blocks failed to resolve.
        if runtime_side() == RuntimeSide::Client {
            self.spores.init();
            return;
        }
        let Some(content) = Content::resolve() else {
            log("exploration: pack content failed to resolve; the mod stays idle");
            return;
        };
        self.content = Some(content);

        // After Trees — the end of the pipeline. The cavern decorates carved
        // cave volume, so it must see final terrain.
        register_worldgen_feature(WorldgenStage::Trees, GEN_CAVERN);
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> Vec<GenWrite> {
        let Some(content) = &self.content else {
            return Vec::new();
        };
        match feature_id {
            GEN_CAVERN => cavern::generate(content, ctx),
            _ => Vec::new(),
        }
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        self.spores.frame(frame);
    }
}

register_mod!(Exploration);
