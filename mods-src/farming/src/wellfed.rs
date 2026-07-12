//! Well Fed: the Farmer's Lunch marker effect's one consequence.
//!
//! The effect KIND is pack data (`effects.json`, behavior `none`); this
//! handler gives it meaning: every positive, event-routed player damage
//! instance is reduced by one half-heart while the effect is active, and
//! never below one half-heart. Routing through `player_damage_pre` keeps the
//! normal deterministic event ordering for every other mod; direct
//! SetHealth-style state changes are deliberately unaffected. Duration
//! refresh (not stacking) and the respawn clear are the engine's ordinary
//! effect lifecycle.

use mod_sdk::*;

const WELL_FED: &str = "farming:well_fed";

pub fn on_player_damage(amount: &mut i32) {
    if *amount <= 1 {
        return;
    }
    if effects_active().iter().any(|e| e.key == WELL_FED) {
        *amount -= 1;
    }
}
