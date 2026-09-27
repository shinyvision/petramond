#![warn(clippy::too_many_lines)]

use mod_sdk::*;

mod cascade;
mod cavern;
mod content;
mod dripstone;
mod fluids;
mod keys;
mod probe;
mod shroom;
mod spores;

use content::Content;
use dripstone::Dripstone;

const GEN_CAVERN: u32 = 1;
const GEN_DRIPSTONE: u32 = 2;
const HOOK_DRIPSTONE: u32 = 1;
const HANDLER_PROJECTILE: u32 = 1;
const HANDLER_PLAYER_DAMAGE: u32 = 2;
const HANDLER_MOB_DAMAGE: u32 = 3;
const HANDLER_PLACED: u32 = 4;

pub(crate) use keys::MUSHROOM_CAVERN as BIOME_KEY;

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
                register_block_behavior(keys::POINTED_DRIPSTONE_HOOK, HOOK_DRIPSTONE);
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
                Some(Err(probe::Deferred)) => GenOutput::deferred(),
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
