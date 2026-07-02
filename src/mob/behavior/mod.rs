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
            WanderAi::new(def.wander, &def.habitat, def.avoid_water),
        )
        .with(PRIORITY_EXPRESSION, HeadLookAi::new())
        .with(PRIORITY_EXPRESSION, IdleAnimAi::new())
}

/// The sheep brain reuses the passive wander + expression stack: it browses through
/// its habitat, avoids water destinations, looks at nearby players while idle, and
/// plays any `idle_*` animations the model may later define.
pub fn sheep_brain(def: &'static MobDef) -> Brain {
    Brain::new()
        .with(
            PRIORITY_WANDER,
            WanderAi::new(def.wander, &def.habitat, def.avoid_water),
        )
        .with(PRIORITY_EXPRESSION, HeadLookAi::new())
        .with(PRIORITY_EXPRESSION, IdleAnimAi::new())
}
