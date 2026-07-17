use serde::{Deserialize, Serialize};

use crate::mathh::{IVec3, Vec3};
use crate::server::player::PlayerId;

use super::{ActionOutcome, ItemSlotWire, MenuSyncMsg, Transform};

/// A world cell changed. `block_id` is a wire block id; `water` the water meta
/// byte when the cell holds water. Coalesced latest-wins per cell per tick,
/// sent only for sections in the recipient's sent set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BlockDelta {
    pub pos: IVec3,
    pub block_id: u8,
    pub water: Option<u8>,
    /// The cell's sparse per-cell block state after the change, `None` when the
    /// cell carries none (the replica then CLEARS any stale state, mirroring
    /// `clear_on_block_change` server-side).
    pub state: Option<CellState>,
}

/// One cell's sparse block state on the wire — the delta-sized sibling of
/// [`SectionStatesPayload`], using the SAME save-codec per-entry encodings
/// (`DoorState::encode`, `StairState::encode`, `SlabState::encode_meta` + raw
/// layer BLOCK IDS, `LogAxis::to_u8`, `TorchPlacement::to_u8`,
/// `Facing::to_u8`). A cell holds at most one of these; an oriented multi-cell
/// model block folds its placed facing into [`CellState::ModelCell`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CellState {
    /// `DoorState::encode` byte (facing + open + which-half).
    Door(u8),
    /// `StairState::encode` byte.
    Stair(u8),
    /// `[SlabState::encode_meta, layer 0 block id, layer 1 block id]` — raw
    /// session block ids, remapped at the transport boundary like
    /// `SectionStatesPayload::slabs`.
    Slab([u8; 3]),
    /// `LogAxis::to_u8` byte.
    LogAxis(u8),
    /// `TorchPlacement::to_u8` byte.
    Torch(u8),
    /// `Facing::to_u8` byte — chest/furnace block-entity fronts.
    Facing(u8),
    /// A multi-cell model block's authored footprint offset + placed facing
    /// (`Facing::to_u8`).
    ModelCell { off: [u8; 3], facing: u8 },
}

/// One live mob's replicated state as of the batch's tick — everything the
/// client's `MobPresentation` needs except light (client-sampled at `pos`).
/// The client store keeps the previous batch's row per id and interpolates
/// prev→curr, exactly as the renderer interpolates `Instance::prev_*`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct MobStateRow {
    /// Stable mob identity (`Instance::id`).
    pub id: u64,
    /// Wire mob id (`Mob.0` on the server).
    pub kind_id: u8,
    pub pos: Vec3,
    pub yaw: f32,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// Hurt-flash SOURCE state (the instance's remaining hurt timer, seconds).
    /// The presentation flash is derived from consecutive rows client-side
    /// (`mob::hurt_flash01`), never shipped pre-derived.
    pub hurt_timer: f32,
    pub dead: bool,
    pub shorn: bool,
    /// ACTIVE particle-emitter bundle ids (wire `particle_emitters.json`
    /// catalog ids, `Instance::active_emitters`, ≤ 4). The client derives the
    /// particle rows and any body tint from its own catalog after the remap,
    /// so a few bytes replicate the whole effect.
    pub emitters: Vec<u8>,
    /// ACTIVE named model animations as `(name, phase)` pairs
    /// (`Instance::active_anims`, ≤ 4, sorted by name): each layer's phase is
    /// SELF-CLOCKED server-side (mods drive the rate), so a paused oar's
    /// phase simply stops advancing and the client interpolates phases
    /// between rows like positions. Names are MODEL-LOCAL (no registry, no
    /// numeric id) — no remap; unknown names draw nothing. Empty for every
    /// mob without mod animations, so the common row pays one length byte.
    pub anims: Vec<(String, f32)>,
    /// Per-bone ragdoll pose (pivot position, orientation quaternion) as of
    /// this tick — present only while the death ragdoll plays (bounded), so
    /// live mobs pay nothing for it.
    pub ragdoll: Option<Vec<([f32; 3], [f32; 4])>>,
}

/// One dropped item entity's replicated state as of the batch's tick — the
/// `DroppedItemPresentation` fields minus light (client-sampled at `pos`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ItemStateRow {
    /// Stable per-spawn identity (`DroppedItem::id`).
    pub id: u64,
    /// Wire item id.
    pub item_id: u8,
    pub count: u8,
    pub pos: Vec3,
    pub spin: f32,
}

/// One connected player's replicated state as of the batch's tick — EVERY
/// session's row is sent to every recipient (bytes are trivial); the client
/// skips its OWN id (the local body renders from the predicted player). Light
/// is client-sampled at `pos`, like mobs and items.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerStateRow {
    pub id: PlayerId,
    /// `pos` is the feet position (the body model's `y = 0`).
    pub transform: Transform,
    pub on_ground: bool,
    pub sneaking: bool,
    pub sleeping: bool,
    /// The lying head yaw while sleeping (base→pillow cell of the session's
    /// bed), computed server-side against the authoritative model group so a
    /// client without that section still poses the sleeper right.
    pub sleep_yaw: Option<f32>,
    pub alive: bool,
    /// False hides the body entirely: spectators and the dead.
    pub visible: bool,
    /// Selected hotbar item (wire item id); `None` for an empty hand.
    pub held_item: Option<u8>,
    /// The in-progress mining target + crack stage (0..=9). Drives the remote
    /// body's looping arm swing (`is_some()`) AND the remote break (crack)
    /// overlay every observer renders. The recipient's OWN crack overlay is
    /// client-owned (its local mining timer) and never ships back to it.
    pub mining: Option<(IVec3, u8)>,
    /// Mid-eat — drives the remote chew pose (progress is approximated
    /// client-side; only the blend/nibble channels pose the body).
    pub eating: bool,
    /// The player took damage this tick window. Sessions track no hurt TIMER
    /// (unlike `MobStateRow::hurt_timer`), so this ships the EDGE and each
    /// client runs its own flash envelope — the same one as the local
    /// third-person body's hurt flash.
    pub hurt_recent: bool,
    /// This tick window TELEPORTED the player (sleep tuck, wake, respawn, mod
    /// teleport — the same transform-drift detection that feeds
    /// `SelfState::transform`): the client snaps interpolation instead of
    /// lerping across the jump.
    pub snap: bool,
    /// The mob this player is riding — `(stable mob id, seat index)` — or
    /// `None`. Clients GLUE a mounted body (their own included) to the
    /// interpolated mob presentation at the species' seat offset instead of
    /// lerping the row transform, so rider and mount can never visibly
    /// separate.
    pub mount: Option<(u64, u8)>,
}

/// One-shot remote-player animation events, broadcast alongside the state
/// rows as `(player, kind)` pairs — the wire form of that session's lossy
/// `PlayerTickEvents` one-shots. No registry ids ride here.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PlayerActionKind {
    Swung,
    Broke,
    Placed,
    ThrewItem,
    UsedItem,
    Interacted,
    AteFinished,
    Died,
    Respawned,
}

/// A server-authoritative transform correction: this pump's fixed ticks moved
/// the recipient's player (bed tuck, wake/respawn teleport, mod `Teleport`,
/// mob-strike knockback) — the session transform no longer matches the last
/// CLIENT-REPORTED one. Carries the full transform; the client adopts the
/// fields that differ from what it last SENT (per-field, so its newer
/// per-frame look/movement is not stomped by an echo of an older frame).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelfTransform {
    pub transform: Transform,
    pub on_ground: bool,
}

/// The recipient's OWN replicated player state: everything the HUD, hand, and
/// overlays read (health/effects/inventory/mining/eating/sleeping). Sent with
/// every `TickUpdate`; the inventory body rides only when its revision moved
/// (and always on the first update after join).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelfState {
    /// Health in half-heart points.
    pub health: i32,
    /// `PlayerMode` as its discriminant (0 = survival, 1 = spectator).
    pub mode: u8,
    /// Active status effects as (wire effect id, remaining ticks), in
    /// application order.
    pub effects: Vec<(u8, u32)>,
    /// The server-side inventory mutation counter `inventory` was sampled at.
    pub inventory_revision: u64,
    /// All 36 slots in index order, then the cursor stack LAST (the
    /// `SelfRestore` layout). `None` while the revision hasn't moved since the
    /// last update the recipient saw.
    ///
    /// The active hotbar INDEX and the own mining overlay deliberately do NOT
    /// ride here: both are client-owned (the index rides `PlayerUpdate`, the
    /// crack overlay is the local timer) — echoing them back would replay or
    /// stomp the client's own newer state.
    pub inventory: Option<Vec<Option<ItemSlotWire>>>,
    /// The in-progress eat's progress, 0-255 over the food's eat time.
    pub eating: Option<u8>,
    /// The in-progress sleep's fade progress, 0-255 (clamped at full).
    pub sleeping: Option<u8>,
    /// The in-progress sleep's bed base (foot) cell — the client derives the
    /// lying body's head yaw from it. `None` while awake.
    pub sleep_bed: Option<IVec3>,
    /// A transform correction when the ticks moved this player (see
    /// [`SelfTransform`]); `None` on ordinary updates.
    pub transform: Option<SelfTransform>,
}

/// One world-anchored event a tick produced, broadcast to every observer
/// (positional presentation: break bursts, door swings, positional sounds).
/// Registry ids (`block_id`/`kind_id`/`sound_id`) are wire ids, remapped at
/// the transport boundary like every other id field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum WorldEventMsg {
    BlockBroken {
        pos: IVec3,
        block_id: u8,
        /// The mined face (directional burst spread), when known.
        normal: Option<IVec3>,
    },
    BlockPlaced {
        pos: IVec3,
        block_id: u8,
    },
    /// A door toggled: the LOWER cell + its NEW open state.
    DoorToggled {
        lower: IVec3,
        open: bool,
    },
    /// A chest's viewer count crossed 0→1 (first screen opened on it).
    ChestOpened {
        pos: IVec3,
    },
    /// A chest's viewer count crossed 1→0 (last screen closed on it).
    ChestClosed {
        pos: IVec3,
    },
    /// A player collected at least one drop this tick, at their body centre.
    ItemPickedUp {
        pos: Vec3,
        by: PlayerId,
    },
    /// A semantic mob sound (hurt/death); the client resolves the species hook.
    MobSound {
        mob_id: u64,
        kind_id: u8,
        /// `MobSoundCategory` discriminant.
        category: u8,
        pos: Vec3,
    },
    /// A mod-emitted one-shot (`EmitSound`); `pos = None` is non-spatial.
    ModSound {
        sound_id: u8,
        pos: Option<Vec3>,
    },
    /// A one-shot particle burst (a `particle_emitters.json` burst bundle by
    /// wire catalog id) at `pos` — e.g. the water splash.
    EmitterBurst {
        emitter_id: u8,
        pos: Vec3,
        intensity: f32,
    },
    /// A mod-owned spatial sound command (`SoundPlayAt`/`OnMob`/`Stop`).
    ModSpatialSound(ModSpatialSoundMsg),
}

/// [`crate::game::ModSpatialSoundCommand`] on the wire (sound ids as wire ids).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ModSpatialSoundMsg {
    PlayAt {
        handle: u64,
        sound_id: u8,
        pos: Vec3,
        volume: f32,
        pitch: f32,
    },
    PlayOnMob {
        handle: u64,
        sound_id: u8,
        mob_id: u64,
        volume: f32,
        pitch: f32,
        /// The mob position at emission (fallback if it despawns client-side).
        last_pos: Vec3,
    },
    Stop {
        handle: u64,
    },
}

/// Which screen the server opened for the recipient this tick (the menu
/// session is already open server-side; the client shows the matching screen).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum OpenScreen {
    /// The inventory screen with inventory-tier recipes.
    Inventory,
    /// The crafting-table screen with inventory- and table-tier recipes.
    CraftingTable,
    Furnace(IVec3),
    Chest(IVec3),
    /// The furniture workbench (its session carries no position).
    Workbench,
    /// A mod GUI; `kind_key` is the registered kind's stable string key
    /// (GuiKind ids are process-local, so the wire speaks keys).
    ModGui {
        kind_key: String,
        pos: Option<IVec3>,
    },
    /// The sleep overlay (bed interaction).
    Sleep,
}

/// The recipient's own lossy per-tick one-shots: hand jabs, hurt/death,
/// screen opens — everything that used to ride `PlayerTickEvents` plus the
/// session's `request_open_*` outbox. `broke_block`/`placed_block` carry wire
/// block ids (they pick the client's hand animation + sound mapping).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelfEvents {
    // The hand-animation one-shots (broke/placed/swung/threw/used/interacted)
    // deliberately do NOT ride here: the recipient initiated those actions and
    // already animated them at click time — echoing them back replays the
    // animation one RTT later. Observers get them via `player_actions`.
    // `used_unpredicted` is the one deliberate exception: it fires ONLY for a
    // consumed click whose `UseClick.jabbed` said the initiator stayed silent
    // (a mod-consumed use/interact the replica cannot foresee), so it can
    // never double an already-played jab.
    pub picked_up_item: bool,
    pub bed_interacted: bool,
    pub player_damaged: bool,
    pub player_died: bool,
    pub sleep_ended: bool,
    pub respawned: bool,
    pub open_screen: Option<OpenScreen>,
    pub close_mod_gui: bool,
    /// The door toggle's NEW open state — only the TOGGLER gets this one-shot
    /// (the world-anchored `DoorToggled` event reaches every observer).
    pub toggled_door: Option<bool>,
    /// A use click was CONSUMED server-side (mod-cancelled item use / block
    /// interact) but the initiator's own jab verdict was silent — play the
    /// hand jab now. See the header note on the no-echo rule.
    pub used_unpredicted: bool,
}

impl SelfEvents {
    /// Fold another batch's one-shots in (booleans OR, options latest-wins) —
    /// used client-side if more than one `TickUpdate` lands in a frame.
    pub(crate) fn merge_from(&mut self, other: SelfEvents) {
        self.picked_up_item |= other.picked_up_item;
        self.bed_interacted |= other.bed_interacted;
        self.player_damaged |= other.player_damaged;
        self.player_died |= other.player_died;
        self.sleep_ended |= other.sleep_ended;
        self.respawned |= other.respawned;
        if other.open_screen.is_some() {
            self.open_screen = other.open_screen;
        }
        self.close_mod_gui |= other.close_mod_gui;
        self.toggled_door = other.toggled_door.or(self.toggled_door);
        self.used_unpredicted |= other.used_unpredicted;
    }
}

/// One executed server tick's replication to one client. At most one per pump
/// (states as of the latest executed tick); the client applies it atomically
/// and interpolates between consecutive updates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TickUpdate {
    pub tick: u64,
    pub clock: u64,
    pub block_deltas: Vec<BlockDelta>,
    /// Every live mob's state (interest scoping lands with per-player
    /// streaming).
    pub mobs: Vec<MobStateRow>,
    /// Every active dropped item's state.
    pub items: Vec<ItemStateRow>,
    /// Every connected session's player state (recipient included — the
    /// client skips its own id).
    pub players: Vec<PlayerStateRow>,
    /// This window's one-shot player animation events, in emission order.
    pub player_actions: Vec<(PlayerId, PlayerActionKind)>,
    /// The recipient's own player state (per-recipient; in-process there is
    /// one recipient — session 0).
    pub self_state: Option<SelfState>,
    /// Every chest with at least one open screen (any player), FULL state per
    /// batch — the `chest_viewers` key set, tiny. Drives every client's lid
    /// animation.
    pub open_chests: Vec<IVec3>,
    /// The server `WorldEnvironment`'s FULL named-shader-param map, present
    /// only when any value changed since the last shipped copy (`None` =
    /// unchanged, keep). Names are strings (engine `petramond:*` keys + mod
    /// namespaces) — no registry ids ride here. The client applies it into
    /// its REPLICA world's environment, which the renderer reads.
    pub env: Option<Vec<(String, [f32; 4])>>,
    /// This tick window's world-anchored events, in emission order.
    pub events: Vec<WorldEventMsg>,
    /// The recipient's own per-tick one-shots.
    pub self_events: SelfEvents,
    /// Answers to this recipient's [`ClientRequestId`]s (menu clicks, breaks,
    /// drops, …), in emission order.
    pub action_outcomes: Vec<ActionOutcome>,
    /// The recipient's menu-session view when it changed (`None` = unchanged).
    pub menu_sync: Option<MenuSyncMsg>,
}
