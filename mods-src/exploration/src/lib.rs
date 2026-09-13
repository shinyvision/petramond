//! Two underground habitats: the mushroom cavern and its cave flora, and the
//! dripstone caves and their spikes.

use mod_sdk::*;

mod cascade;
mod cavern;
mod content;
mod dripstone;
mod fluids;
mod shroom;
mod spores;

use content::Content;
use dripstone::Dripstone;

/// Feature ids echoed back to [`Mod::gen_feature`].
const GEN_CAVERN: u32 = 1;
const GEN_DRIPSTONE: u32 = 2;
/// Block-behaviour callback for both spike rows.
const HOOK_DRIPSTONE: u32 = 1;
/// Event handlers.
const HANDLER_PROJECTILE: u32 = 1;
const HANDLER_PLAYER_DAMAGE: u32 = 2;
const HANDLER_MOB_DAMAGE: u32 = 3;
const HANDLER_PLACED: u32 = 4;

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
    dripstone: Option<Dripstone>,
    spores: spores::Spores,
}

impl Mod for Exploration {
    fn init(&mut self) {
        if runtime_side() == RuntimeSide::Client {
            self.spores.init();
            return;
        }
        // After Trees — the end of the pipeline. Both habitats decorate
        // carved cave volume, so they must see final terrain.
        let fluids = fluids::Fluids::resolve();
        match Content::resolve(fluids.clone()) {
            Some(content) => {
                self.content = Some(content);
                register_worldgen_feature(WorldgenStage::Trees, GEN_CAVERN, cavern::GEN_FILTER);
            }
            None => {
                log("exploration: mushroom content failed to resolve; mushroom decoration disabled")
            }
        }
        match Dripstone::resolve(fluids) {
            Some(dripstone) => {
                self.dripstone = Some(dripstone);
                register_worldgen_feature(
                    WorldgenStage::Trees,
                    GEN_DRIPSTONE,
                    dripstone::gen::GEN_FILTER,
                );
                register_block_behavior("exploration:pointed_dripstone", HOOK_DRIPSTONE);
                register_event_handler(EventKind::ProjectileHit, 0, HANDLER_PROJECTILE);
                register_event_handler(EventKind::PlayerDamagePre, 0, HANDLER_PLAYER_DAMAGE);
                register_event_handler(EventKind::MobDamagePre, 0, HANDLER_MOB_DAMAGE);
                register_event_handler(EventKind::BlockPlaced, 0, HANDLER_PLACED);
            }
            None => log("exploration: dripstone content failed to resolve; dripstone disabled"),
        }
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> GenOutput {
        match feature_id {
            GEN_CAVERN => match self.content.as_ref().map(|c| cavern::generate(c, ctx)) {
                Some(Ok(writes)) => writes.into(),
                Some(Err(cavern::Deferred)) => GenOutput::deferred(),
                None => GenOutput::default(),
            },
            GEN_DRIPSTONE => self
                .dripstone
                .as_ref()
                .map_or_else(GenOutput::default, |d| {
                    dripstone::gen::generate(d, ctx).into()
                }),
            _ => GenOutput::default(),
        }
    }

    fn block_hook(&mut self, callback_id: u32, kind: BlockHookKind, pos: [i32; 3]) {
        if let (HOOK_DRIPSTONE, Some(d)) = (callback_id, &self.dripstone) {
            dripstone::behavior::on_hook(d, kind, pos);
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        let Some(d) = &self.dripstone else {
            return Outcome::Continue;
        };
        match handler_id {
            HANDLER_PROJECTILE => dripstone::hazard::on_projectile_hit(payload),
            HANDLER_PLAYER_DAMAGE => dripstone::hazard::on_player_damage(d, payload),
            HANDLER_MOB_DAMAGE => dripstone::hazard::on_mob_damage(d, payload),
            HANDLER_PLACED => dripstone::behavior::on_placed(d, payload),
            _ => Outcome::Continue,
        }
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        self.spores.frame(frame);
    }
}

register_mod!(Exploration);
