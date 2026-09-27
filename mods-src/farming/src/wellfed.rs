use mod_sdk::*;

use crate::keys::WELL_FED;

pub fn on_player_damage(amount: &mut i32) {
    if *amount <= 1 {
        return;
    }
    if effects_active().iter().any(|e| e.key == WELL_FED) {
        *amount -= 1;
    }
}
