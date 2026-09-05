//! The tick→presentation event vocabulary: what one fixed tick did, split per
//! audience. Produced by the authoritative sim (`server::game`) and the
//! modding host; consumed by replication (the wire) and every client's
//! presentation. The sim never touches audio or particles — it only queues
//! these values.

use crate::player::PlayerId;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::Block;

/// Fixed simulation timestep: 20 game ticks per second, independent of frame
/// rate. World simulation (block updates, scheduled ticks, water flow) advances
/// in whole steps of this size.
pub const TICK_DT: f32 = 0.05;

/// One sound a mod emitted on the tick (`EmitSound` HostCall): resolved to a
/// runtime [`Sound`](petramond_world::sound_registry::Sound) id at call time, carried through the
/// tick→presentation channel, and played by the app layer each frame — the sim
/// never touches audio. `pos` is where it happened (`None` = non-spatial);
/// positional reach comes from the sound row's `attenuation_distance`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SoundEvent {
    pub sound: petramond_world::sound_registry::Sound,
    pub pos: Option<petramond_math::math::Vec3>,
}

/// A semantic mob sound event produced by gameplay. The app resolves the
/// species' `mobs.json` sound hook and owns actual playback.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobSoundEvent {
    pub mob_id: u64,
    pub kind: crate::mob::Mob,
    pub category: crate::mob::MobSoundCategory,
    pub pos: petramond_math::math::Vec3,
}

/// A deterministic presentation command produced by the spatial sound HostCalls.
/// The app/audio side owns actual playback and active sinks; the sim only carries
/// resolved sound ids, stable handles, and positions through the tick event queue.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SpatialSoundCommand {
    PlayAt {
        handle: u64,
        sound: petramond_world::sound_registry::Sound,
        pos: petramond_math::math::Vec3,
        volume: f32,
        pitch: f32,
    },
    PlayOnMob {
        handle: u64,
        sound: petramond_world::sound_registry::Sound,
        mob_id: u64,
        volume: f32,
        pitch: f32,
        /// The mob position when the command was emitted. If the mob despawns
        /// before the app sees a frame snapshot, playback starts and finishes here.
        last_pos: petramond_math::math::Vec3,
    },
    Stop {
        handle: u64,
    },
    /// Retune a live handle in place (`SoundSet`).
    Set {
        handle: u64,
        volume: f32,
        pitch: f32,
    },
}

/// The per-PLAYER slice of what the tick did: the lossy latched one-shots that
/// feed that player's `GameEvents` (hand jabs, hurt shake, screen requests).
/// One per session per tick; the acting session's slice is written by the
/// per-player stages.
#[derive(Copy, Clone, Debug, Default)]
pub struct PlayerTickEvents {
    pub broke_block: Option<Block>,
    pub placed_block: Option<Block>,
    pub swung_hand: bool,
    pub picked_up_item: bool,
    pub threw_item: bool,
    pub used_item: bool,
    /// An eat COMPLETED this tick (the food was consumed) — as opposed to the
    /// level `eating` state ending in an abort. Feeds the remote-player
    /// `AteFinished` action; the local client's presentation reads the eat
    /// progress instead.
    pub ate_finished: bool,
    pub bed_interacted: bool,
    pub interacted: bool,
    pub player_damaged: bool,
    pub player_died: bool,
    pub sleep_ended: bool,
    pub respawned: bool,
    /// The door toggle's NEW open state, latched for the TOGGLER only.
    pub toggled_door: Option<bool>,
    /// A use click was consumed but the initiator's own jab verdict
    /// (`UseClick::jabbed`) was silent — echo the hand jab back to them
    /// (`SelfEvents::used_unpredicted`). Observers are unaffected (they get
    /// `used_item`/`interacted` via the shared action rows).
    pub used_unpredicted: bool,
    /// This tick's use click was claimed on the ladder's OFF-hand pass, so its
    /// one-shots (`placed_block`/`used_item`/`interacted`/`used_unpredicted`)
    /// animate the left hand. At most one click dispatches per tick, so one
    /// flag covers them all; the eat completion keeps its own flag because an
    /// eat can finish in the same tick as an unrelated click.
    pub click_off_hand: bool,
    /// The completed eat (`ate_finished`) consumed from the OFF hand.
    pub ate_off_hand: bool,
}

/// One keyed event addressed at a single player's CLIENT instance
/// (`EmitEventTo`): a namespaced key and an opaque payload, carried to that
/// session's replication batch and dispatched into its client runtime.
///
/// Per-player but NON-lossy, which is why it rides here rather than in
/// [`PlayerTickEvents`]: those are latched one-shots the newest overwrites,
/// and cues are a queue nobody may silently drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientEvent {
    pub player: PlayerId,
    pub key: String,
    pub data: Vec<u8>,
}

/// A block the sim destroyed this tick (player-mined or natural), with
/// everything a CLIENT needs to present it: break-burst particles at `pos`,
/// sampled against the post-tick world. Position-carrying and broadcastable —
/// the wire replicates these to every client in range.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockBrokenEvent {
    pub pos: IVec3,
    pub block: Block,
    /// The mined face (for directional burst spread), when known.
    pub normal: Option<IVec3>,
    /// The cell's `petramond:tint` KV at break time (see
    /// `WorldEvent::BlockBroken`).
    pub tint: Option<[u8; 3]>,
}

/// The WORLD-anchored slice of what the tick did: non-lossy queues every
/// observer cares about, independent of which player acted. `sounds`/
/// `spatial_sounds`/`mob_sounds` are the existing presentation feeds;
/// `block_broken`/`door_changed` are consumed client-side
/// after the tick (particles, swing/lid animation seeds) and become broadcast
/// messages when the wire exists.
#[derive(Clone, Debug)]
pub struct WorldEvents {
    pub sounds: Vec<SoundEvent>,
    pub spatial_sounds: Vec<SpatialSoundCommand>,
    pub mob_sounds: Vec<MobSoundEvent>,
    pub block_broken: Vec<BlockBrokenEvent>,
    /// A block placed by a player: (anchor cell, block).
    pub block_placed: Vec<(IVec3, Block)>,
    /// A door toggled: (lower cell, new open state).
    pub door_changed: Vec<(IVec3, bool)>,
    /// A chest's viewer count crossed 0↔1: (chest cell, now open).
    pub chest_changed: Vec<(IVec3, bool)>,
    /// A player collected at least one drop: (their body centre, player id).
    pub item_picked_up: Vec<(Vec3, PlayerId)>,
    /// One-shot particle bursts (catalog id, world position, producer-defined
    /// intensity — the water splash passes blocks fallen).
    pub emitter_bursts: Vec<(u8, Vec3, f32)>,
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
            door_changed: Vec::new(),
            chest_changed: Vec::new(),
            item_picked_up: Vec::new(),
            emitter_bursts: Vec::new(),
            next_spatial_sound_handle: next_spatial_sound_handle.max(1),
        }
    }
}

/// What the world-mutating actions did across the fixed tick(s) that ran this frame.
/// The tick→presentation channel: the event bus feeds it (via `SimCtx::feed`),
/// never the other way around. Crate-visible so event handlers can write it.
/// Split per audience: `players[s]` is player `s`'s lossy one-shot slice,
/// `world` the shared non-lossy queues.
#[derive(Clone, Debug)]
pub struct TickEvents {
    players: Vec<PlayerTickEvents>,
    pub world: WorldEvents,
    /// Cues addressed at ONE player's client (`EmitEventTo`), in emission
    /// order. A third audience beside "this player's one-shots" and "everyone
    /// in range": addressed like the former, queued like the latter.
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

    /// Player `s`'s event slice, grown on demand so tests (and late joins mid-
    /// frame) never index out of bounds.
    pub fn player(&mut self, s: usize) -> &mut PlayerTickEvents {
        if self.players.len() <= s {
            self.players.resize_with(s + 1, Default::default);
        }
        &mut self.players[s]
    }

    /// Read-only copy of player `s`'s slice (default if nothing was written).
    pub fn player_at(&self, s: usize) -> PlayerTickEvents {
        self.players.get(s).copied().unwrap_or_default()
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
