use super::{Player, PlayerMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerAbilities {
    pub flight: bool,
    pub flying: bool,
    pub noclip: bool,
    pub invulnerable: bool,
    pub free_placement: bool,
    pub instant_break: bool,
    pub yields_drops: bool,
    pub restricted_items: bool,
    pub item_catalog: bool,
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
