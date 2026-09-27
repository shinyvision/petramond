use crate::player::PlayerId;
use petramond_math::math::IVec3;
use petramond_world::block::Block;

pub use crate::world::TICK_DT;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SoundEvent {
    pub sound: petramond_world::sound_registry::Sound,
    pub pos: Option<petramond_math::world_pos::WorldPos>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobSoundEvent {
    pub mob_id: u64,
    pub kind: crate::mob::Mob,
    pub category: crate::mob::MobSoundCategory,
    pub pos: petramond_math::world_pos::WorldPos,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SpatialSoundCommand {
    PlayAt {
        handle: u64,
        sound: petramond_world::sound_registry::Sound,
        pos: petramond_math::world_pos::WorldPos,
        volume: f32,
        pitch: f32,
    },
    PlayOnMob {
        handle: u64,
        sound: petramond_world::sound_registry::Sound,
        mob_id: u64,
        volume: f32,
        pitch: f32,
        last_pos: petramond_math::world_pos::WorldPos,
    },
    Stop {
        handle: u64,
    },
    Set {
        handle: u64,
        volume: f32,
        pitch: f32,
    },
}

#[derive(Clone, Debug, Default)]
pub struct PlayerTickEvents {
    pub broke_block: Option<Block>,
    pub placed_block: Option<Block>,
    pub swung_hand: bool,
    pub picked_up_item: bool,
    pub threw_item: bool,
    pub used_item: bool,
    pub bed_interacted: bool,
    pub interacted: bool,
    pub player_damaged: bool,
    pub player_died: bool,
    pub sleep_ended: bool,
    pub respawned: bool,
    pub toggled_panel: Option<bool>,
    pub used_unpredicted: bool,
    pub click_off_hand: bool,
    pub animator_events: Vec<(crate::player::RigId, u16)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientEvent {
    pub player: PlayerId,
    pub key: String,
    pub data: Vec<u8>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockBrokenEvent {
    pub pos: IVec3,
    pub block: Block,
    pub normal: Option<IVec3>,
    pub tint: Option<[u8; 3]>,
}

#[derive(Clone, Debug)]
pub struct WorldEvents {
    pub sounds: Vec<SoundEvent>,
    pub spatial_sounds: Vec<SpatialSoundCommand>,
    pub mob_sounds: Vec<MobSoundEvent>,
    pub block_broken: Vec<BlockBrokenEvent>,
    pub block_placed: Vec<(IVec3, Block)>,
    pub panel_changed: Vec<(IVec3, bool)>,
    pub chest_changed: Vec<(IVec3, bool)>,
    pub item_picked_up: Vec<(petramond_math::world_pos::WorldPos, PlayerId)>,
    pub emitter_bursts: Vec<BurstFired>,
    next_spatial_sound_handle: u64,
}

impl WorldEvents {
    fn with_next_spatial_sound_handle(next_spatial_sound_handle: u64) -> Self {
        Self {
            sounds: Vec::new(),
            spatial_sounds: Vec::new(),
            mob_sounds: Vec::new(),
            block_broken: Vec::new(),
            block_placed: Vec::new(),
            panel_changed: Vec::new(),
            chest_changed: Vec::new(),
            item_picked_up: Vec::new(),
            emitter_bursts: Vec::new(),
            next_spatial_sound_handle: next_spatial_sound_handle.max(1),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TickEvents {
    players: Vec<PlayerTickEvents>,
    pub world: WorldEvents,
    pub client_events: Vec<ClientEvent>,
}

impl Default for TickEvents {
    fn default() -> Self {
        Self::with_next_spatial_sound_handle(1)
    }
}

impl TickEvents {
    pub fn with_next_spatial_sound_handle(next_spatial_sound_handle: u64) -> Self {
        Self {
            players: Vec::new(),
            world: WorldEvents::with_next_spatial_sound_handle(next_spatial_sound_handle),
            client_events: Vec::new(),
        }
    }

    pub fn player(&mut self, s: usize) -> &mut PlayerTickEvents {
        if self.players.len() <= s {
            self.players.resize_with(s + 1, Default::default);
        }
        &mut self.players[s]
    }

    pub fn player_at(&self, s: usize) -> &PlayerTickEvents {
        static NONE: std::sync::LazyLock<PlayerTickEvents> =
            std::sync::LazyLock::new(PlayerTickEvents::default);
        self.players.get(s).unwrap_or(&NONE)
    }

    pub fn next_spatial_sound_handle(&self) -> u64 {
        self.world.next_spatial_sound_handle
    }

    pub fn alloc_spatial_sound_handle(&mut self) -> u64 {
        let handle = self.world.next_spatial_sound_handle.max(1);
        self.world.next_spatial_sound_handle = handle.wrapping_add(1).max(1);
        handle
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BurstFired {
    pub emitter: u8,
    pub pos: petramond_math::world_pos::WorldPos,
    pub intensity: f32,
    pub direction: Option<[f32; 3]>,
    pub texture: Option<BurstTexture>,
}

impl BurstFired {
    pub fn plain(emitter: u8, pos: petramond_math::world_pos::WorldPos, intensity: f32) -> Self {
        Self {
            emitter,
            pos,
            intensity,
            direction: None,
            texture: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BurstTexture {
    Tile {
        slice: petramond_world::particle_emitters::TextureSlice,
        tint: [u8; 3],
    },
    Block {
        block: petramond_world::block::Block,
        tint: Option<[u8; 3]>,
    },
}
