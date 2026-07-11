//! The wire protocol: everything a client and server say to each other.
//!
//! Messages are plain Rust values passed UNSERIALIZED over the in-process
//! connection; the TCP transport postcard-encodes them on its own threads.
//! Registry ids on the wire are RAW session ids (`u8`/`u16` fields named
//! `*_id`); the TCP client remaps them against the [`JoinData::tables`] name
//! tables at the transport boundary (see `net::remap`) — everything above the
//! transport speaks purely client-local ids. The local connection skips the
//! remap entirely (same process, same registries).
//!
//! Wire-compat: break freely and bump [`super::PROTOCOL_VERSION`] — nothing is
//! released, so there are no old clients to keep decoding an older dialect.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::{IVec3, Vec3};
use crate::server::player::PlayerId;

pub(crate) const MAX_CHAT_CHARS: usize = 256;

/// A shared byte buffer on the wire: refcount-bumped over the local
/// connection, serialized as plain bytes over TCP (deserialization allocates a
/// fresh `Arc`, which the remap then rewrites in place — no extra copies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SectionBytes(pub Arc<[u8]>);

impl Serialize for SectionBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for SectionBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'a> serde::de::Visitor<'a> for V {
            type Value = SectionBytes;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a byte buffer")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<SectionBytes, E> {
                Ok(SectionBytes(Arc::from(v)))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<SectionBytes, E> {
                Ok(SectionBytes(Arc::from(v.into_boxed_slice())))
            }
            fn visit_seq<A: serde::de::SeqAccess<'a>>(
                self,
                mut seq: A,
            ) -> Result<SectionBytes, A::Error> {
                let mut v = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    v.push(b);
                }
                Ok(SectionBytes(Arc::from(v.into_boxed_slice())))
            }
        }
        d.deserialize_bytes(V)
    }
}

/// One enabled mod, as the handshake reports it. Version is display-only —
/// compatibility checks are by ID (see WIKI/multiplayer.md).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModEntry {
    pub id: String,
    pub version: String,
}

/// The small fixed chat palette understood by clients. Messages carry
/// structured spans, not inline control text, so clients never parse player
/// content as formatting.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ChatColor {
    White,
    Red,
    Yellow,
    Blue,
    Cyan,
}

/// One styled text run in a chat line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChatSpan {
    pub fg: ChatColor,
    pub text: String,
}

/// One server-accepted chat line. Sequence numbers are session-local and only
/// provide a stable ordering key for clients/tests; chat history is not
/// retained server-side or replayed to later joiners.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChatLine {
    pub seq: u64,
    pub spans: Vec<ChatSpan>,
}

/// Why a `Join` was refused. Kept for protocol stability (clients still handle
/// a reject) — the server no longer emits `NameTaken`: a duplicate name is
/// auto-deduped with a numeric suffix at admission instead (see
/// `ServerGame::admit_remote_player`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum JoinRejectReason {
    /// A connected player already uses this name (case-insensitive).
    NameTaken,
}

/// The server's registry name tables, in server-runtime-id order — the wire's
/// id vocabulary (the on-disk analogue is `palette.json`). The client builds
/// server-id→client-id LUTs from these at join.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NameTables {
    pub blocks: Vec<String>,
    pub items: Vec<String>,
    pub mobs: Vec<String>,
    pub sounds: Vec<String>,
    pub effects: Vec<String>,
    /// `particle_emitters.json` bundle keys, in server-id order.
    pub emitters: Vec<String>,
}

/// One inventory slot on the wire. `item_id` is a wire item id.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ItemSlotWire {
    pub item_id: u8,
    pub count: u8,
}

/// The joining player's own restored state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelfRestore {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// `PlayerMode` as its discriminant (tiny closed enum).
    pub mode: u8,
    pub health: i32,
    pub bed_spawn: Option<(IVec3, IVec3)>,
    /// Active status effects by registry NAME + remaining ticks (names, not
    /// ids: effects are the one registry small enough that names are cheap and
    /// they already persist by name in level.dat).
    pub effects: Vec<(String, u32)>,
    /// All inventory slots in index order, then the cursor stack last.
    pub inventory: Vec<Option<ItemSlotWire>>,
    /// The active hotbar slot, so the restored selection survives the join.
    pub active_slot: u8,
}

/// Everything a client needs to enter the world, sent on `JoinAccept`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct JoinData {
    pub player_id: PlayerId,
    pub seed: u32,
    /// The day/night clock (`petramond:clock` contract) so the sky renders right
    /// from the first frame.
    pub clock: u64,
    pub tables: NameTables,
    pub self_restore: SelfRestore,
    /// Already-connected players (id, name), local player excluded.
    pub players: Vec<(PlayerId, String)>,
}

/// Per-connection monotonic id for discrete mutating intents that need an
/// [`ActionOutcome`]. Client allocates; server echoes.
pub(crate) type ClientRequestId = u32;

/// Coarse deny reasons for [`ActionOutcome`] — enough for rollback/UI.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ActionDenyReason {
    OutOfReach,
    InvalidSlot,
    Busy,
    Denied,
    TooFast,
    BadTool,
}

/// Server answer to one client request id (accept or deny).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ActionOutcome {
    pub id: ClientRequestId,
    pub accepted: bool,
    pub reason: Option<ActionDenyReason>,
}

/// Per-frame (local) / throttled (TCP) client transform + held intents.
///
/// Movement F2: `wishdir` / `jump` / `sprint` are the authoritative input; the
/// server integrates physics on the fixed tick. `pos`/`vel`/`on_ground` remain
/// the client's prediction (used for soft comparison / fall bookkeeping until
/// a hard correct ships via [`SelfTransform`]).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerUpdate {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub sneak: bool,
    /// Gameplay input live (false while a screen owns focus — server forces
    /// held intents off, mirroring `capture_intent`).
    pub gameplay: bool,
    pub break_held: bool,
    pub use_held: bool,
    /// The client's raycast target (block + face normal), reach-validated
    /// server-side.
    pub target: Option<TargetRef>,
    pub hotbar_slot: u8,
    pub held_rotation: u8,
    /// Horizontal/3D wish direction (unit or zero) for server-side movement.
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TargetRef {
    pub block: IVec3,
    pub normal: IVec3,
}

/// One-shot player actions, applied in arrival order on the next server tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum PlayerAction {
    /// Secondary press: the interact/eat/use/place ladder. `mob` is the mob
    /// under the crosshair at click time (stable id) — the shear target, like
    /// `AttackClick`'s. `target` is the block under the crosshair AT CLICK
    /// TIME: the server resolves the interact/place against THIS cell, never
    /// a fresher look latch — otherwise a click racing the crosshair places
    /// somewhere the client's ghost isn't. `request_id` is set when the
    /// client opened a ledger entry (place ghost or track-only);
    /// presentation-only jabs may omit it. `predicted` says whether the
    /// client actually PRESENTED a full place (ghost + sound) — it gates the
    /// initiator's `BlockPlaced` echo strip only, exactly like
    /// `BreakFinished.predicted`: an unpredicted placement (oriented model,
    /// replace-in-place, slab stack, frozen ledger) must keep its event or
    /// the initiator never hears their own place.
    UseClick {
        mob: Option<u64>,
        target: Option<TargetRef>,
        request_id: Option<ClientRequestId>,
        predicted: bool,
    },
    /// Primary press: attack the mob under the crosshair (stable mob id), the
    /// remote PLAYER under the crosshair (`PlayerId` byte — PvP), or punch the
    /// air. The client sends AT MOST ONE of `mob`/`player` (targeting picks
    /// the nearest); the server validates the player target (alive,
    /// non-spectator, within reach) before any damage.
    AttackClick {
        mob: Option<u64>,
        player: Option<u8>,
    },
    Drop {
        all: bool,
        request_id: ClientRequestId,
    },
    ThrowCursorStack {
        request_id: ClientRequestId,
    },
    ThrowCursorOne {
        request_id: ClientRequestId,
    },
    /// Client finished mining locally; server validates tool/reach and the
    /// duration against ITS OWN observed mining window (never client-reported
    /// time — see WIKI/client-prediction.md).
    BreakFinished {
        request_id: ClientRequestId,
        pos: IVec3,
        /// Wire item id of the tool used (`None` = bare hand).
        tool_item_id: Option<u8>,
        /// Whether the client applied the break optimistically (replica clear
        /// + local sound/burst). Gates the initiator's echo strip: a
        /// track-only finish (frozen ledger, replica disagreement) never
        /// presented, so its `BlockBroken` world event must still be
        /// delivered. Presentation-only — the validation path ignores it.
        predicted: bool,
    },
    Wake,
    Respawn,
    /// Request a survival/spectator toggle (Ctrl+Y). Applied at message time
    /// only when the sending session is an operator; the session's fall
    /// tracker re-anchors so the switch is never measured as a fall. The
    /// authoritative mode flows back via [`SelfState::mode`].
    ToggleMode,
    /// The inventory key (E): the server opens the 2×2 crafting session on the
    /// next tick and answers with [`OpenScreen::Inventory`].
    OpenInventory,
    CloseMenu,
}

/// A container-menu slot identity on the wire — the message twin of
/// [`crate::gui::MenuSlot`], self-contained (widget ids travel as strings; the
/// server re-interns them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MenuSlotWire {
    Inventory(u32),
    CraftInput(u32),
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
    Chest(u32),
    WorkbenchInput,
    WorkbenchResult(u32),
    Container(u32),
    Widget(String),
}

impl MenuSlotWire {
    pub(crate) fn from_menu_slot(slot: &crate::gui::MenuSlot) -> Self {
        use crate::gui::{CraftHit, FurnaceHit, MenuSlot, WorkbenchHit};
        match slot {
            MenuSlot::Inventory(i) => Self::Inventory(*i as u32),
            MenuSlot::Craft(CraftHit::Input(i)) => Self::CraftInput(*i as u32),
            MenuSlot::Craft(CraftHit::Result) => Self::CraftResult,
            MenuSlot::Furnace(FurnaceHit::Input) => Self::FurnaceInput,
            MenuSlot::Furnace(FurnaceHit::Fuel) => Self::FurnaceFuel,
            MenuSlot::Furnace(FurnaceHit::Output) => Self::FurnaceOutput,
            MenuSlot::Chest(i) => Self::Chest(*i as u32),
            MenuSlot::Workbench(WorkbenchHit::Input) => Self::WorkbenchInput,
            MenuSlot::Workbench(WorkbenchHit::Result(i)) => Self::WorkbenchResult(*i as u32),
            MenuSlot::Container(i) => Self::Container(*i as u32),
            MenuSlot::Widget(id) => Self::Widget((*id).to_string()),
        }
    }

    pub(crate) fn to_menu_slot(&self) -> crate::gui::MenuSlot {
        use crate::gui::{CraftHit, FurnaceHit, MenuSlot, WorkbenchHit};
        match self {
            Self::Inventory(i) => MenuSlot::Inventory(*i as usize),
            Self::CraftInput(i) => MenuSlot::Craft(CraftHit::Input(*i as usize)),
            Self::CraftResult => MenuSlot::Craft(CraftHit::Result),
            Self::FurnaceInput => MenuSlot::Furnace(FurnaceHit::Input),
            Self::FurnaceFuel => MenuSlot::Furnace(FurnaceHit::Fuel),
            Self::FurnaceOutput => MenuSlot::Furnace(FurnaceHit::Output),
            Self::Chest(i) => MenuSlot::Chest(*i as usize),
            Self::WorkbenchInput => MenuSlot::Workbench(WorkbenchHit::Input),
            Self::WorkbenchResult(i) => MenuSlot::Workbench(WorkbenchHit::Result(*i as usize)),
            Self::Container(i) => MenuSlot::Container(*i as usize),
            Self::Widget(id) => MenuSlot::Widget(crate::gui::intern_str(id)),
        }
    }
}

/// [`crate::controls::PointerButton`] on the wire.
pub(crate) fn button_to_wire(button: crate::controls::PointerButton) -> u8 {
    match button {
        crate::controls::PointerButton::Primary => 0,
        crate::controls::PointerButton::Secondary => 1,
    }
}

pub(crate) fn button_from_wire(button: u8) -> crate::controls::PointerButton {
    match button {
        0 => crate::controls::PointerButton::Primary,
        _ => crate::controls::PointerButton::Secondary,
    }
}

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
    /// Feet position (the body model's `y = 0`).
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
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
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
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
    /// The inventory screen with its 2×2 crafting grid.
    Inventory,
    /// The 3×3 crafting-table screen.
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
    }
}

/// One [`crate::gui::GuiValue`] on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum GuiValueWire {
    F32(f32),
    I32(i32),
    Str(String),
}

impl GuiValueWire {
    pub(crate) fn from_value(v: &crate::gui::GuiValue) -> Self {
        match v {
            crate::gui::GuiValue::F32(x) => Self::F32(*x),
            crate::gui::GuiValue::I32(x) => Self::I32(*x),
            crate::gui::GuiValue::Str(s) => Self::Str(s.clone()),
        }
    }

    pub(crate) fn into_value(self) -> crate::gui::GuiValue {
        match self {
            Self::F32(x) => crate::gui::GuiValue::F32(x),
            Self::I32(x) => crate::gui::GuiValue::I32(x),
            Self::Str(s) => crate::gui::GuiValue::Str(s),
        }
    }
}

/// The recipient's open menu-session target, with everything its screen
/// renders. Item slots are wire ids ([`ItemSlotWire`]), remapped at the
/// transport boundary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) enum MenuTargetWire {
    /// No menu session is open (sent once when a session closes).
    #[default]
    None,
    /// The inventory's 2×2 crafting session (grid rides `craft_grid`).
    Inventory,
    /// The crafting table's 3×3 session.
    Table,
    Furnace {
        pos: IVec3,
        /// `[input, fuel, output]`.
        slots: [Option<ItemSlotWire>; 3],
        cook01: f32,
        burn01: f32,
    },
    Chest {
        pos: IVec3,
        slots: Vec<Option<ItemSlotWire>>,
    },
    Workbench {
        input: Option<ItemSlotWire>,
        /// `(wire item id, craftable now)` per offered recipe, row-major.
        results: Vec<(u8, bool)>,
    },
    ModGui {
        kind_key: String,
        pos: Option<IVec3>,
        /// The backing container's slots, `None` for a slot-less GUI.
        slots: Option<Vec<Option<ItemSlotWire>>>,
        /// The session's full state map — present ONLY when it changed since
        /// the last sync (`Arc` identity check server-side); `None` = keep.
        gui_state: Option<Vec<(String, GuiValueWire)>>,
    },
}

/// The recipient's menu-session view, sent inside a `TickUpdate` only when it
/// changed since the last one this session was sent (value compare; the
/// `gui_state` map compares by `Arc` identity).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MenuSyncMsg {
    pub target: MenuTargetWire,
    /// The open crafting grid's cells (`cols²` entries; empty when no crafting
    /// session is up).
    pub craft_grid: Vec<Option<ItemSlotWire>>,
    pub craft_result: Option<ItemSlotWire>,
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

/// Client → server messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ClientToServer {
    /// First frame on any fresh connection; nothing else parses across a
    /// protocol mismatch.
    Hello {
        protocol: u16,
    },
    /// "Which mods do you have?" — asked (and answered) BEFORE joining, per
    /// the mod-handshake contract.
    ModQuery,
    Join {
        player_name: String,
        /// The client's view distance in chunks. The server streams
        /// `min(this, its own maximum)` for the session.
        view_distance: u8,
    },
    PlayerUpdate(PlayerUpdate),
    Action(PlayerAction),
    /// A hit-tested container-menu click (slot identity + button + Shift +
    /// the client's double-click gather verdict), latched to the next tick.
    MenuClick {
        slot: MenuSlotWire,
        button: u8,
        shift: bool,
        gather: bool,
        request_id: ClientRequestId,
    },
    /// A player-submitted chat line. The server trims/sanitizes/formats it and
    /// broadcasts the resulting [`ServerToClient::ChatLine`].
    ChatSend {
        text: String,
    },
    /// Acknowledge one streaming batch (`StreamBatchStart`..`StreamBatchEnd`)
    /// and report the rate this client actually applied it at — the
    /// end-to-end flow-control signal the server sizes future batches from
    /// (the 1.20.2 chunk-batching design; see WIKI/multiplayer.md).
    StreamBatchAck {
        messages_per_second: f32,
    },
    /// The client changed its view distance (Options → Graphics). The server
    /// re-clamps to its own maximum and streams the new radius; terrain
    /// outside it unloads client-side through the ordinary diff.
    SetViewDistance {
        chunks: u8,
    },
    Pause(bool),
    KeepAlive,
    Disconnect,
}

/// Server → client messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ServerToClient {
    HelloAck {
        protocol: u16,
    },
    HelloReject {
        server_protocol: u16,
    },
    /// The hosted world's ENABLED mod set (`modding/modset.rs::active`).
    ModList {
        mods: Vec<ModEntry>,
    },
    JoinAccept(Box<JoinData>),
    JoinReject {
        reason: JoinRejectReason,
    },
    ColumnData(ColumnPayload),
    /// Boxed like [`Tick`](Self::Tick): the payload's sparse state vecs make
    /// it by far the largest variant, and it dominates channel traffic during
    /// a world load.
    SectionData(Box<SectionPayload>),
    /// A server light bake landed for a section already sent to this
    /// recipient: the fresh cubes replace the seeded ones. The replica never
    /// bakes its own light — this is the ONLY post-install light writer.
    LightData(LightPayload),
    SectionUnload(SectionPos),
    ColumnUnload(ChunkPos),
    /// Brackets the start of one streaming batch (terrain/light/unload
    /// messages) on a WINDOWED connection: the client times Start→End
    /// application and answers `StreamBatchAck`. Loopback uses the same
    /// protocol with a one-batch window, bounding its unbounded channel.
    StreamBatchStart,
    /// Ends the batch `StreamBatchStart` opened; `count` is the number of
    /// streaming messages in between (the client's rate denominator).
    StreamBatchEnd {
        count: u32,
    },
    Tick(Box<TickUpdate>),
    PlayerJoined {
        id: PlayerId,
        name: String,
    },
    PlayerLeft {
        id: PlayerId,
    },
    ChatLine(ChatLine),
    ServerClosing,
    KeepAlive,
    Disconnect {
        reason: String,
    },
}

/// A column's client-relevant facts: the biome skin, the heightmap, and a
/// per-cy section summary so replica physics can answer for ABSENT sections
/// without running worldgen. Sent before the column's first section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ColumnPayload {
    pub pos: ChunkPos,
    /// 16×16 biome ids, row-major (z * 16 + x).
    pub biomes: SectionBytes,
    /// 20x20 biome tint halo (two cells beyond each column edge), captured by
    /// column generation and reused by every section mesh in this column.
    pub mesh_biomes: SectionBytes,
    /// 16×16 surface heights, same order.
    pub heightmap: Vec<i32>,
    /// `SectionSummary` discriminants for every cy in world order — lets the
    /// replica treat absent `FullOpaque`/`FullWater` sections truthfully.
    pub summaries: Vec<u8>,
    /// Lowest section in the surface retention band. Sections below it are
    /// eligible for replica deep-visibility parking.
    pub deep_band_lo: i32,
}

/// One 16³ section's full streamed content — the wire sibling of the save's
/// `SectionSnapshot`, Arc-backed so the local connection ships refcount bumps.
/// Container SLOT contents, mobs, and dropped items are deliberately absent:
/// they replicate through menu sync and entity batches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SectionPayload {
    pub pos: SectionPos,
    /// 4096 wire block ids.
    pub blocks: SectionBytes,
    /// Block-derived counters and boundary planes. The replica adopts these
    /// with the shared buffers instead of rescanning the section on its frame.
    pub metrics: crate::section::SectionMetrics,
    /// 4096 water meta bytes, present when any cell holds water.
    pub water: Option<SectionBytes>,
    /// Server-baked light. The ship gate (`plan_terrain_send`) holds a section
    /// back until its light is final, so this is `None` ONLY for sections that
    /// never bake (fully opaque) — the replica does no light work of its own.
    /// Post-install rebakes arrive as [`LightData`](ServerToClient::LightData).
    pub skylight: Option<SectionBytes>,
    pub blocklight: Option<SectionBytes>,
    /// Sparse per-cell block states (doors, stairs, slabs, log axes, torches,
    /// saplings, model cells, facings, lit furnaces, cell KV).
    pub states: SectionStatesPayload,
}

/// One section's freshly baked light cubes — shipped whenever a server bake
/// lands for a section in the recipient's sent set (rebakes after edits and
/// after a neighbour's landing invalidated a seam). Arc-backed like
/// [`SectionPayload`]: the local pipe ships refcount bumps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct LightPayload {
    pub pos: SectionPos,
    /// 4096 skylight bytes (x2 scale).
    pub skylight: SectionBytes,
    /// 4096 block-light bytes; `None` when no emitter reaches the section
    /// (reads as all-zero, mirroring `Section::set_blocklight`'s compaction).
    pub blocklight: Option<SectionBytes>,
}

/// The sparse per-cell state maps a section carries beyond raw block ids.
/// Cell keys are the section-local u16 cell index; every entry list is sorted
/// by cell so identical state encodes identically. Encodings are EXACTLY the
/// save codec's per-entry bytes (`save::codec::encode_snapshot`) — the wire
/// delegates to the same `encode`/`to_u8` state packers, so replication is as
/// lossless as a save/load roundtrip. Built/consumed by `world::remote`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SectionStatesPayload {
    /// (cell, `DoorState::encode` byte)
    pub doors: Vec<(u16, u8)>,
    /// (cell, `StairState::encode` byte)
    pub stairs: Vec<(u16, u8)>,
    /// (cell, [`SlabState::encode_meta`, layer 0 block id, layer 1 block id])
    /// — the save codec's 3-byte record, with RAW session block ids.
    pub slabs: Vec<(u16, [u8; 3])>,
    /// (cell, `LogAxis::to_u8` byte)
    pub log_axes: Vec<(u16, u8)>,
    /// (cell, `TorchPlacement::to_u8` byte)
    pub torches: Vec<(u16, u8)>,
    /// (cell, sapling growth stage)
    pub saplings: Vec<(u16, u8)>,
    /// (cell, `Facing::to_u8` byte) — chest/furnace block-entity fronts.
    pub entity_facings: Vec<(u16, u8)>,
    /// (cell, `Facing::to_u8` byte) — oriented bbmodel blocks.
    pub model_facings: Vec<(u16, u8)>,
    /// (cell, authored footprint offset) for multi-cell model blocks.
    pub model_cells: Vec<(u16, [u8; 3])>,
    /// Cells whose furnace is LIT. Machine state (burn/cook counters) is sim
    /// state and stays server-side; the replica only needs the lit face.
    pub furnaces_lit: Vec<u16>,
    /// Per-cell mod KV, preserved opaquely (entries sorted by key — the map
    /// is a `BTreeMap` section-side).
    pub cell_kv: Vec<CellKvEntry>,
}

/// One cell's opaque mod KV: `(cell, sorted (key, value-bytes) entries)` —
/// the wire mirror of the section's per-cell `BTreeMap`.
pub(crate) type CellKvEntry = (u16, Vec<(String, Vec<u8>)>);

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let bytes = postcard::to_allocvec(v).expect("encode");
        let back: T = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(&back, v);
    }

    #[test]
    fn representative_messages_roundtrip_through_postcard() {
        roundtrip(&ClientToServer::Hello { protocol: 1 });
        roundtrip(&ClientToServer::Join {
            player_name: "Rachel".into(),
            view_distance: 16,
        });
        roundtrip(&ClientToServer::SetViewDistance { chunks: 24 });
        roundtrip(&ClientToServer::PlayerUpdate(PlayerUpdate {
            pos: Vec3::new(1.5, 80.0, -3.25),
            vel: Vec3::ZERO,
            yaw: 1.25,
            pitch: -0.5,
            on_ground: true,
            sneak: false,
            gameplay: true,
            break_held: true,
            use_held: false,
            target: Some(TargetRef {
                block: IVec3::new(4, 63, -2),
                normal: IVec3::new(0, 1, 0),
            }),
            hotbar_slot: 3,
            held_rotation: 1,
            wishdir: Vec3::ZERO,
            jump: false,
            sprint: false,
        }));
        roundtrip(&ClientToServer::Action(PlayerAction::UseClick {
            mob: Some(812),
            target: Some(TargetRef {
                block: IVec3::new(4, 65, -2),
                normal: IVec3::Y,
            }),
            request_id: Some(7),
            predicted: true,
        }));
        roundtrip(&ClientToServer::Action(PlayerAction::AttackClick {
            mob: None,
            player: Some(2),
        }));
        roundtrip(&ClientToServer::MenuClick {
            slot: MenuSlotWire::Widget("kitchen:cook".into()),
            button: 0,
            shift: false,
            gather: true,
            request_id: 3,
        });
        roundtrip(&ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 9,
            pos: IVec3::new(1, 2, 3),
            tool_item_id: None,
            predicted: true,
        }));
        roundtrip(&ClientToServer::ChatSend {
            text: "hello server".into(),
        });
        roundtrip(&ActionOutcome {
            id: 1,
            accepted: false,
            reason: Some(ActionDenyReason::TooFast),
        });
        roundtrip(&ServerToClient::ModList {
            mods: vec![ModEntry {
                id: "kitchen".into(),
                version: "0.1.0".into(),
            }],
        });
        roundtrip(&ServerToClient::ChatLine(ChatLine {
            seq: 9,
            spans: vec![
                ChatSpan {
                    fg: ChatColor::Yellow,
                    text: "Rachel".into(),
                },
                ChatSpan {
                    fg: ChatColor::White,
                    text: " joined".into(),
                },
            ],
        }));
        roundtrip(&ServerToClient::JoinReject {
            reason: JoinRejectReason::NameTaken,
        });
    }

    #[test]
    fn arc_backed_section_payloads_roundtrip_byte_exact() {
        let blocks: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let payload = SectionPayload {
            pos: SectionPos {
                cx: -3,
                cy: 2,
                cz: 17,
            },
            blocks: SectionBytes(Arc::from(blocks.into_boxed_slice())),
            metrics: Default::default(),
            water: None,
            skylight: None,
            blocklight: None,
            states: SectionStatesPayload {
                doors: vec![(4095, 7)],
                slabs: vec![(9, [5, 3, 0])],
                model_cells: vec![(80, [1, 0, 1])],
                entity_facings: vec![(7, 2)],
                furnaces_lit: vec![7],
                cell_kv: vec![(12, vec![("kitchen:burn".into(), vec![1, 2, 3])])],
                ..Default::default()
            },
        };
        let bytes = postcard::to_allocvec(&ServerToClient::SectionData(Box::new(payload.clone())))
            .expect("encode");
        let back: ServerToClient = postcard::from_bytes(&bytes).expect("decode");
        let ServerToClient::SectionData(got) = back else {
            panic!("variant preserved");
        };
        assert_eq!(*got, payload);
        // The local path never serializes: cloning the message bumps the Arc.
        let cloned = payload.clone();
        assert!(Arc::ptr_eq(&cloned.blocks.0, &payload.blocks.0));
    }

    #[test]
    fn tick_updates_roundtrip() {
        roundtrip(&ServerToClient::Tick(Box::new(TickUpdate {
            tick: 812,
            clock: 6_600,
            block_deltas: vec![
                BlockDelta {
                    pos: IVec3::new(-8, 70, 3),
                    block_id: 9,
                    water: Some(0x87),
                    state: None,
                },
                BlockDelta {
                    pos: IVec3::new(4, 65, 4),
                    block_id: 12,
                    water: None,
                    state: Some(CellState::Slab([1, 12, 0])),
                },
                BlockDelta {
                    pos: IVec3::new(5, 65, 4),
                    block_id: 30,
                    water: None,
                    state: Some(CellState::ModelCell {
                        off: [1, 0, 0],
                        facing: 3,
                    }),
                },
            ],
            mobs: vec![MobStateRow {
                id: 4211,
                kind_id: 1,
                pos: Vec3::new(4.5, 71.0, -2.25),
                yaw: 0.75,
                anim_time: 12.5,
                moving: true,
                idle_anim: Some(1),
                head_yaw: -0.25,
                head_pitch: 0.1,
                hurt_timer: 0.2,
                dead: false,
                shorn: true,
                emitters: vec![1],
                ragdoll: Some(vec![([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0])]),
            }],
            items: vec![ItemStateRow {
                id: 7,
                item_id: 3,
                count: 12,
                pos: Vec3::new(0.5, 65.0, 0.5),
                spin: 1.25,
            }],
            players: vec![PlayerStateRow {
                id: PlayerId(1),
                pos: Vec3::new(4.5, 71.0, -2.25),
                vel: Vec3::new(0.0, -0.5, 1.0),
                yaw: 0.75,
                pitch: -0.25,
                on_ground: true,
                sneaking: false,
                sleeping: true,
                sleep_yaw: Some(1.5),
                alive: true,
                visible: true,
                held_item: Some(5),
                mining: Some((IVec3::new(4, 70, -2), 6)),
                eating: false,
                hurt_recent: true,
                snap: true,
            }],
            player_actions: vec![
                (PlayerId(1), PlayerActionKind::Broke),
                (PlayerId(0), PlayerActionKind::AteFinished),
            ],
            self_state: Some(SelfState {
                health: 14,
                mode: 0,
                effects: vec![(0, 900)],
                inventory_revision: 42,
                inventory: Some(vec![
                    Some(ItemSlotWire {
                        item_id: 5,
                        count: 64,
                    }),
                    None,
                ]),
                eating: Some(128),
                sleeping: None,
                sleep_bed: None,
                transform: Some(SelfTransform {
                    pos: Vec3::new(1.5, 80.0, -3.25),
                    vel: Vec3::ZERO,
                    yaw: 1.25,
                    pitch: -0.5,
                    on_ground: true,
                }),
            }),
            open_chests: vec![IVec3::new(1, 65, 1)],
            env: Some(vec![
                ("petramond:time".into(), [0.5, 1.0, 3.0, 0.0]),
                ("petramond:light".into(), [1.0, 1.0, 1.0, 1.0]),
            ]),
            events: vec![
                WorldEventMsg::BlockBroken {
                    pos: IVec3::new(4, 65, 4),
                    block_id: 12,
                    normal: Some(IVec3::Y),
                },
                WorldEventMsg::ItemPickedUp {
                    pos: Vec3::new(1.0, 65.0, 2.0),
                    by: PlayerId(1),
                },
                WorldEventMsg::ModSpatialSound(ModSpatialSoundMsg::PlayOnMob {
                    handle: 3,
                    sound_id: 2,
                    mob_id: 4211,
                    volume: 0.5,
                    pitch: 1.1,
                    last_pos: Vec3::new(0.0, 70.0, 0.0),
                }),
            ],
            self_events: SelfEvents {
                picked_up_item: true,
                open_screen: Some(OpenScreen::ModGui {
                    kind_key: "kitchen:oven".into(),
                    pos: Some(IVec3::new(4, 65, 4)),
                }),
                ..Default::default()
            },
            action_outcomes: vec![ActionOutcome {
                id: 1,
                accepted: true,
                reason: None,
            }],
            menu_sync: Some(MenuSyncMsg {
                target: MenuTargetWire::ModGui {
                    kind_key: "kitchen:oven".into(),
                    pos: Some(IVec3::new(4, 65, 4)),
                    slots: Some(vec![
                        Some(ItemSlotWire {
                            item_id: 5,
                            count: 3,
                        }),
                        None,
                    ]),
                    gui_state: Some(vec![("kitchen:burn01".into(), GuiValueWire::F32(0.5))]),
                },
                craft_grid: Vec::new(),
                craft_result: None,
            }),
        })));
    }
}
