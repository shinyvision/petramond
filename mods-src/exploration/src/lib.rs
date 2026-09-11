//! The mushroom cavern: an underground habitat and its cave flora.

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

/// Top of the depth band that row declares (`"y": [-64, -8]`). Nothing this
/// pack places can be rooted above it, so worldgen derives its altitude gate
/// from here — retuning the band moves this one value with the JSON.
pub const BIOME_TOP_Y: i32 = 96;

#[derive(Default)]
struct Exploration {
    content: Option<Content>,
    spores: spores::Spores,
}

impl Mod for Exploration {
    fn init(&mut self) {
        if runtime_side() == RuntimeSide::Client {
            self.spores.init();
            return;
        }
        let Some(content) = Content::resolve() else {
            log("exploration: mushroom content failed to resolve; mushroom decoration disabled");
            return;
        };
        self.content = Some(content);

        // After Trees — the end of the pipeline. The cavern decorates carved
        // cave volume, so it must see final terrain.
        register_worldgen_feature(WorldgenStage::Trees, GEN_CAVERN, cavern::GEN_FILTER);
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> GenOutput {
        match feature_id {
            GEN_CAVERN => match self.content.as_ref().map(|c| cavern::generate(c, ctx)) {
                Some(Ok(writes)) => writes.into(),
                Some(Err(cavern::Deferred)) => GenOutput::deferred(),
                None => GenOutput::default(),
            },
            _ => GenOutput::default(),
        }
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        self.spores.frame(frame);
    }
}

register_mod!(Exploration);
