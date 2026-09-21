//! What each [`PlayerMode`] permits. Rules ask for the ability they mean;
//! this table is the only place a mode is translated into behaviour.

use super::{Player, PlayerMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerAbilities {
    /// May switch between walking and collision-bound flight.
    pub flight: bool,
    /// Currently flying rather than walking.
    pub flying: bool,
    /// Moves through the world without colliding or interacting.
    pub noclip: bool,
    /// Takes no damage, knockback or exposure.
    pub invulnerable: bool,
    /// Placing a block keeps the held stack.
    pub free_placement: bool,
    /// A break request lands at once, whatever the block's hardness or tool.
    pub instant_break: bool,
    /// A broken block scatters its drops and contents.
    pub yields_drops: bool,
    /// May place items that exist only for operators.
    pub restricted_items: bool,
    /// The inventory key opens the item catalog.
    pub item_catalog: bool,
    /// May apply bulk cell edits, and every edit enters the edit history.
    pub edits_cells: bool,
}

const SURVIVAL: PlayerAbilities = PlayerAbilities {
    flight: false,
    flying: false,
    noclip: false,
    invulnerable: false,
    free_placement: false,
    instant_break: false,
    yields_drops: true,
    restricted_items: false,
    item_catalog: false,
    edits_cells: false,
};

const SPECTATOR: PlayerAbilities = PlayerAbilities {
    noclip: true,
    invulnerable: true,
    ..SURVIVAL
};

const CREATIVE: PlayerAbilities = PlayerAbilities {
    flight: true,
    invulnerable: true,
    free_placement: true,
    instant_break: true,
    yields_drops: false,
    restricted_items: true,
    item_catalog: true,
    edits_cells: true,
    ..SURVIVAL
};

impl PlayerMode {
    pub const fn abilities(self) -> PlayerAbilities {
        match self {
            PlayerMode::Survival => SURVIVAL,
            PlayerMode::Spectator => SPECTATOR,
            PlayerMode::Creative => CREATIVE,
            PlayerMode::CreativeFlying => PlayerAbilities {
                flying: true,
                ..CREATIVE
            },
        }
    }
}

impl Player {
    #[inline]
    pub fn abilities(&self) -> PlayerAbilities {
        self.mode().abilities()
    }
}
