//! Camp skeletons: guards that man a camp's posts and fight with whatever their hands hold.
//!
//! - [`garrison`] finds the posts as camp sections stream in, puts a guard on every empty one
//!   (out of the players' sight) and dresses each skeleton the session has not dressed yet.
//! - [`kit`] reads the loadouts and the items' `monsters:wield` rows.
//! - [`combat`] runs the fight each tick: sight, then the melee swing or the bow.
//! - [`guard`] is the shield's raise-and-lower clock, and refuses the hits a raised shield faces.
//! - [`leash`] is the brain node that walks an idle guard back to its post.
//! - [`standards`] knows which camps still fly their skull flag, when the flags pack is loaded:
//!   only those camps' posts fill.
//!
//! What a skeleton carries and where it stands are tags (saved with the body); everything else
//! here is session memory keyed by the mob's id.

mod aim;
mod combat;
mod garrison;
mod geometry;
mod guard;
mod keys;
mod kit;
mod leash;
mod posts;
mod presence;
mod standards;

#[cfg(test)]
mod tests;

use mod_sdk::*;

use combat::Fight;
use kit::Kits;
use posts::Posts;
use presence::Presence;
use standards::Standards;

use crate::routes::{AiNode, Handler, TickSystem};

pub struct Skeletons {
    kind: MobId,
    kits: Kits,
    posts: Posts,
    /// `None` without the flags pack: every camp keeps its garrison.
    standards: Option<Standards>,
    bodies: FxHashMap<u64, Body>,
    /// Extra shoves owed by hits landed this tick, applied when each hit is seen to go through.
    shoves: Vec<(PlayerId, [f32; 3])>,
}

/// One skeleton as this session knows it.
pub struct Body {
    loadout: Option<usize>,
    post: Option<[i32; 3]>,
    watch: bool,
    presence: Presence,
    fight: Fight,
}

impl Skeletons {
    /// `None` off the tick instance, or when no skeleton row is loaded.
    pub fn init() -> Option<Skeletons> {
        if runtime_side() != RuntimeSide::Server {
            return None;
        }
        let kind = resolve_mob_logged(keys::SKELETON)?;
        let kits = Kits::load(kind);
        register_tick_system(
            Stage::Mobs,
            AttachSide::After,
            0,
            TickSystem::SkeletonCombat.id(),
        );
        register_tick_system(
            Stage::ItemPhysics,
            AttachSide::Before,
            0,
            TickSystem::ShoveReset.id(),
        );
        register_tick_system(
            Stage::Spawning,
            AttachSide::After,
            0,
            TickSystem::Garrison.id(),
        );
        register_event_handler(
            EventKind::SectionGenerated,
            0,
            Handler::SectionGenerated.id(),
        );
        register_event_handler(EventKind::SectionLoaded, 0, Handler::SectionLoaded.id());
        let only_skeletons = EventFilter {
            mobs: vec![kind],
            ..EventFilter::default()
        };
        register_event_handler_filtered(
            EventKind::MobDamagePre,
            0,
            Handler::SkeletonDamaged.id(),
            only_skeletons.clone(),
        );
        register_event_handler_filtered(
            EventKind::MobDied,
            0,
            Handler::SkeletonDied.id(),
            only_skeletons,
        );
        // Last, so it only ever sees a hit nothing refused.
        register_event_handler(
            EventKind::PlayerDamagePre,
            i32::MAX,
            Handler::PlayerDamaged.id(),
        );
        register_ai_node(keys::POST_NODE, AiNode::SkeletonPost.id());
        let names: Vec<&str> = kits.loadouts.iter().map(|l| l.name.as_str()).collect();
        let standards = Standards::init();
        log(&format!(
            "skeletons: {} loadouts ({}){}",
            names.len(),
            names.join(", "),
            if standards.is_some() {
                ", camps hold while their skull flag stands"
            } else {
                ""
            }
        ));
        Some(Skeletons {
            kind,
            kits,
            posts: Posts::default(),
            standards,
            bodies: FxHashMap::default(),
            shoves: Vec::new(),
        })
    }

    /// Dresses the skeletons near players this session has not met yet, then runs their fight.
    pub fn tick_combat(&mut self) {
        let engagement = combat::Engagement::now(self.kind);
        let strangers = engagement.strangers(&self.bodies);
        garrison::enlist(self, strangers);
        combat::tick(self, &engagement);
    }

    pub fn tick_garrison(&mut self) {
        garrison::tick(self);
    }

    /// Forgets the shoves owed by last tick's hits.
    pub fn reset_shoves(&mut self) {
        self.shoves.clear();
    }

    /// Queues a generated or loaded section to be searched for posts.
    pub fn on_section(&mut self, payload: &EventPayload) {
        if let EventPayload::SectionGenerated { pos } | EventPayload::SectionLoaded { pos } =
            payload
        {
            self.posts.queue(*pos, current_tick());
        }
    }

    pub fn on_died(&mut self, payload: &EventPayload) {
        if let EventPayload::MobDied { id, .. } = payload {
            if let Some(post) = self.bodies.remove(id).and_then(|b| b.post) {
                self.posts.record_death(post, current_tick());
            }
        }
    }

    /// A raised shield refuses the hits it faces.
    pub fn on_damage(&mut self, payload: &EventPayload) -> Outcome {
        guard::on_damage(self, payload)
    }

    /// Adds a landed hit's extra shove.
    pub fn on_player_damage(&mut self, payload: &EventPayload) -> Outcome {
        combat::on_player_damage(self, payload)
    }

    pub fn post_node(&self, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        leash::decide(ctx)
    }
}
