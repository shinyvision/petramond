//! The monsters pack: hostile spawning, sunburn, skeleton camps and the skeletons that guard
//! them. Each subsystem registers its own callbacks under a `routes` id; the [`Mod`] impl below
//! only routes each call back to its subsystem.

mod camp;
mod day_clock;
mod daylight;
mod keys;
mod post_marker;
mod routes;
mod skeleton;
mod spawning;
mod sunburn;

use mod_sdk::*;

use routes::{AiNode, Feature, Handler, Spawner, TickSystem};

#[derive(Default)]
struct Monsters {
    camps: Option<camp::Camps>,
    sunburn: sunburn::Sunburn,
    spawner: spawning::Spawner,
    skeletons: Option<skeleton::Skeletons>,
}

impl Mod for Monsters {
    fn init(&mut self) {
        self.camps = camp::Camps::new();
        if self.camps.is_some() {
            register_worldgen_feature(WorldgenStage::Trees, Feature::Camps.id(), camp::GEN_FILTER);
        }
        self.spawner = spawning::Spawner::init();
        self.sunburn = sunburn::Sunburn::init(self.spawner.zombie());
        self.skeletons = skeleton::Skeletons::init();
        log(&format!(
            "initialized: hostile spawner (zombie + hushjaw) + sunburn, {} spawn-proof surfaces",
            self.spawner.spawn_proof_surfaces()
        ));
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> GenOutput {
        match (Feature::from_id(feature_id), self.camps.as_mut()) {
            (Some(Feature::Camps), Some(camps)) => camps.generate(ctx),
            _ => GenOutput::default(),
        }
    }

    fn gen_claims(&mut self, feature_id: u32, ctx: &ClaimsCtx) -> GenClaims {
        match (Feature::from_id(feature_id), self.camps.as_mut()) {
            (Some(Feature::Camps), Some(camps)) => camps.claims(ctx),
            _ => GenClaims::default(),
        }
    }

    fn tick_system(&mut self, system_id: u32) {
        let Some(system) = TickSystem::from_id(system_id) else {
            return;
        };
        let skeletons = self.skeletons.as_mut();
        match system {
            TickSystem::Sunburn => self.sunburn.tick(),
            TickSystem::SkeletonCombat => skeletons.into_iter().for_each(|s| s.tick_combat()),
            TickSystem::Garrison => skeletons.into_iter().for_each(|s| s.tick_garrison()),
            TickSystem::ShoveReset => skeletons.into_iter().for_each(|s| s.reset_shoves()),
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        let Some(handler) = Handler::from_id(handler_id) else {
            return Outcome::Continue;
        };
        let skeletons = self.skeletons.as_mut();
        match handler {
            Handler::WeatherFeed => self.sunburn.observe(payload),
            Handler::SectionGenerated | Handler::SectionLoaded => {
                skeletons.into_iter().for_each(|s| s.on_section(payload));
            }
            Handler::SkeletonDied => skeletons.into_iter().for_each(|s| s.on_died(payload)),
            Handler::SkeletonDamaged => {
                return skeletons.map_or(Outcome::Continue, |s| s.on_damage(payload));
            }
            Handler::PlayerDamaged => {
                return skeletons.map_or(Outcome::Continue, |s| s.on_player_damage(payload));
            }
        }
        Outcome::Continue
    }

    fn ai_node(&mut self, callback_id: u32, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        match AiNode::from_id(callback_id)? {
            AiNode::SkeletonPost => self.skeletons.as_ref()?.post_node(ctx),
        }
    }

    fn hostile_spawn_candidate(
        &mut self,
        callback_id: u32,
        candidate: &HostileSpawnCandidate,
    ) -> Option<String> {
        match Spawner::from_id(callback_id)? {
            Spawner::Hostiles => self.spawner.candidate(candidate),
        }
    }
}

register_mod!(Monsters);
