//! Mob AI behaviors — one composable unit each, à la the block behaviors.
//!
//! A species' `make_brain` fn composes its behaviors into a [`Brain`](super::Brain)
//! with priorities. Adding a behavior (flee, attack) is: add a file, add its `mod` +
//! `pub use`, and drop it into the relevant species brains at a priority — no change
//! to the brain's arbitration or the navigator.

mod head_look;
mod idle_anim;
mod wander;

pub use head_look::HeadLookAi;
pub use idle_anim::IdleAnimAi;
pub use wander::WanderAi;

use super::brain::{Brain, PRIORITY_EXPRESSION, PRIORITY_WANDER};
use super::MobDef;

/// Per-tick chance an idle owl picks a new wander destination. ~1/80 at 20 TPS ≈ a
/// new stroll every few seconds of standing around.
const OWL_WANDER_CHANCE: f32 = 1.0 / 80.0;
/// Owl wander radius (blocks) — the destination is sampled within this of the owl.
const OWL_WANDER_RADIUS: i32 = 8;

/// The owl's brain: gentle ground wandering biased by its [`Habitat`](super::Habitat)
/// (read off the row), plus the expressive idle behaviors — glancing around /
/// watching the player, and (if the model had any) playing `idle_*` animations. A
/// future "flee from player" behavior would slot in at [`PRIORITY_FLEE`] above wander.
///
/// [`PRIORITY_FLEE`]: super::brain::PRIORITY_FLEE
pub fn owl_brain(def: &'static MobDef) -> Brain {
    Brain::new()
        .with(
            PRIORITY_WANDER,
            WanderAi::new(
                OWL_WANDER_CHANCE,
                OWL_WANDER_RADIUS,
                &def.habitat,
                def.avoid_water,
            ),
        )
        .with(PRIORITY_EXPRESSION, HeadLookAi::new())
        .with(PRIORITY_EXPRESSION, IdleAnimAi::new())
}
