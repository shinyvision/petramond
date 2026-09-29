//! Camp skeletons: guards that man a camp's posts and fight with whatever their hands hold.
//!
//! - [`garrison`] finds the posts as camp sections stream in, puts a guard on every empty one
//!   (out of the players' sight) and dresses each skeleton the session has not dressed yet.
//! - [`kit`] reads the loadouts and the items' `monsters:wield` rows.
//! - [`combat`] runs the fight each tick: sight, then the melee swing or the bow.
//! - [`guard`] is the shield's raise-and-lower clock, and refuses the hits a raised shield faces.
//! - [`leash`] is the brain node that walks an idle guard back to its post.
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

#[cfg(test)]
mod tests;

use mod_sdk::*;

use combat::Fight;
use kit::Kits;
use posts::Posts;
use presence::Presence;

const COMBAT_SYSTEM: u32 = 100;
const GARRISON_SYSTEM: u32 = 101;
const SHOVE_RESET_SYSTEM: u32 = 102;

const ON_SECTION_GENERATED: u32 = 100;
const ON_SECTION_LOADED: u32 = 101;
const ON_DAMAGE: u32 = 102;
const ON_DIED: u32 = 103;
const ON_PLAYER_DAMAGE: u32 = 104;

const POST_NODE_CALLBACK: u32 = 100;

pub struct Skeletons {
    kind: MobId,
    kits: Kits,
    posts: Posts,
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
        register_tick_system(Stage::Mobs, AttachSide::After, 0, COMBAT_SYSTEM);
        register_tick_system(
            Stage::ItemPhysics,
            AttachSide::Before,
            0,
            SHOVE_RESET_SYSTEM,
        );
        register_tick_system(Stage::Spawning, AttachSide::After, 0, GARRISON_SYSTEM);
        register_event_handler(EventKind::SectionGenerated, 0, ON_SECTION_GENERATED);
        register_event_handler(EventKind::SectionLoaded, 0, ON_SECTION_LOADED);
        let only_skeletons = EventFilter {
            mobs: vec![kind],
            ..EventFilter::default()
        };
        register_event_handler_filtered(
            EventKind::MobDamagePre,
            0,
            ON_DAMAGE,
            only_skeletons.clone(),
        );
        register_event_handler_filtered(EventKind::MobDied, 0, ON_DIED, only_skeletons);
        // Last, so it only ever sees a hit nothing refused.
        register_event_handler(EventKind::PlayerDamagePre, i32::MAX, ON_PLAYER_DAMAGE);
        register_ai_node(keys::POST_NODE, POST_NODE_CALLBACK);
        let names: Vec<&str> = kits.loadouts.iter().map(|l| l.name.as_str()).collect();
        log(&format!(
            "skeletons: {} loadouts ({})",
            names.len(),
            names.join(", ")
        ));
        Some(Skeletons {
            kind,
            kits,
            posts: Posts::default(),
            bodies: FxHashMap::default(),
            shoves: Vec::new(),
        })
    }

    pub fn tick(&mut self, system_id: u32) -> bool {
        match system_id {
            COMBAT_SYSTEM => combat::tick(self),
            GARRISON_SYSTEM => garrison::tick(self),
            SHOVE_RESET_SYSTEM => self.shoves.clear(),
            _ => return false,
        }
        true
    }

    pub fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Option<Outcome> {
        let outcome = match (handler_id, &*payload) {
            (ON_SECTION_GENERATED, EventPayload::SectionGenerated { pos })
            | (ON_SECTION_LOADED, EventPayload::SectionLoaded { pos }) => {
                self.posts.queue(*pos, current_tick());
                Outcome::Continue
            }
            (ON_DIED, EventPayload::MobDied { id, .. }) => {
                if let Some(post) = self.bodies.remove(id).and_then(|b| b.post) {
                    self.posts.record_death(post, current_tick());
                }
                Outcome::Continue
            }
            (ON_DAMAGE, _) => guard::on_damage(self, payload),
            (ON_PLAYER_DAMAGE, _) => combat::on_player_damage(self, payload),
            (ON_SECTION_GENERATED | ON_SECTION_LOADED | ON_DIED, _) => Outcome::Continue,
            _ => return None,
        };
        Some(outcome)
    }

    pub fn ai_node(&self, callback_id: u32, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        (callback_id == POST_NODE_CALLBACK)
            .then(|| leash::decide(ctx))
            .flatten()
    }
}
