use serde::{Deserialize, Serialize};

use crate::player::PlayerId;
use petramond_math::math::{IVec3, Tilt};

use super::{ItemSlotWire, Transform};

pub use crate::world::replication::{BlockDelta, BlockDrawDelta, CellKvDelta};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MobStateRow {
    pub id: u64,
    pub kind_id: u8,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub hurt_timer: f32,
    pub dead: bool,
    pub shorn: bool,
    pub emitters: Vec<u8>,
    pub conditions: Vec<(u8, u8)>,
    pub anims: Vec<(String, f32)>,
    pub ragdoll: Option<Vec<([f32; 3], [f32; 4])>>,
    pub dig: Option<(petramond_math::math::IVec3, u8)>,
    pub held: [Option<u16>; 2],
    pub draw: crate::world::draw::BodyDraw,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemStateRow {
    pub id: u64,
    pub item_id: u16,
    pub count: u8,
    pub data: Option<Vec<u8>>,
    pub pos: petramond_math::world_pos::WorldPos,
    pub spin: f32,
    pub flight: Option<[f32; 3]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerStateRow {
    pub conditions: Vec<(u8, u8)>,
    pub id: PlayerId,
    pub transform: Transform,
    pub on_ground: bool,
    pub sneaking: bool,
    pub sleeping: bool,
    pub sleep_yaw: Option<f32>,
    pub alive: bool,
    pub visible: bool,
    pub held_item: Option<u16>,
    pub held_data: Option<Vec<u8>>,
    pub off_hand_item: Option<u16>,
    pub off_hand_data: Option<Vec<u8>>,
    pub mining: Option<(IVec3, u8)>,
    pub eating: bool,
    pub eating_off_hand: bool,
    pub held_pose_main: Option<mod_api::HeldPose>,
    pub held_pose_off: Option<mod_api::HeldPose>,
    pub held_display: [Option<u16>; 2],
    pub bone_poses: Vec<crate::player::BonePose>,
    pub animator: crate::player::AnimatorClaims,
    pub hurt_recent: bool,
    pub snap: bool,
    pub mount: Option<PlayerMount>,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlayerMount {
    Mob {
        id: u64,
        seat: u8,
    },
    Anchor {
        pos: petramond_math::world_pos::WorldPos,
        yaw: f32,
        pose: u8,
    },
}

impl PlayerMount {
    pub fn from_mount(m: crate::mob::riding::Mount) -> Self {
        match m.target {
            crate::mob::riding::MountTarget::Mob(id) => PlayerMount::Mob { id, seat: m.seat },
            crate::mob::riding::MountTarget::Anchor(a) => PlayerMount::Anchor {
                pos: a.pos,
                yaw: a.yaw,
                pose: a.pose,
            },
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerActionKind {
    Died,
    Respawned,
    Animator {
        rig: crate::player::RigId,
        event: u16,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfTransform {
    pub transform: Transform,
    pub on_ground: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfState {
    pub conditions: Vec<(u8, u8)>,
    pub health: i32,
    pub mode: u8,
    pub effects: Vec<(u8, u32)>,
    pub inventory_revision: u64,
    /// All 36 slots in index order, then the cursor stack, then the off-hand
    /// stack LAST (the `SelfRestore` layout). `None` while the revision hasn't
    /// moved since the last update the recipient saw.
    ///
    /// The active hotbar INDEX and the own mining overlay deliberately do NOT
    /// ride here: both are client-owned (the index rides `PlayerUpdate`, the
    /// crack overlay is the local timer) — echoing them back would replay or
    /// stomp the client's own newer state.
    pub inventory: Option<Vec<Option<ItemSlotWire>>>,
    pub eating: Option<u8>,
    pub eating_off_hand: bool,
    pub sleeping: Option<u8>,
    pub sleep_bed: Option<IVec3>,
    pub move_scale: f32,
    pub fly_scale: f32,
    pub denied_actions: crate::player::DeniedActions,
    pub held_pose_main: Option<mod_api::HeldPose>,
    pub held_pose_off: Option<mod_api::HeldPose>,
    pub held_display: [Option<u16>; 2],
    pub bone_poses: Vec<crate::player::BonePose>,
    pub animator: crate::player::AnimatorClaims,
    pub transform: Option<SelfTransform>,
}

impl SelfState {
    pub fn over(self, older: Option<SelfState>) -> SelfState {
        let inventory = self.inventory.or_else(|| older.and_then(|o| o.inventory));
        SelfState {
            inventory,
            transform: None,
            ..self
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorldEventMsg {
    BlockBroken {
        pos: IVec3,
        block_id: u16,
        normal: Option<IVec3>,
        tint: Option<[u8; 3]>,
    },
    BlockPlaced {
        pos: IVec3,
        block_id: u16,
    },
    PanelToggled {
        anchor: IVec3,
        open: bool,
    },
    ChestOpened {
        pos: IVec3,
    },
    ChestClosed {
        pos: IVec3,
    },
    ItemPickedUp {
        pos: petramond_math::world_pos::WorldPos,
        by: PlayerId,
    },
    MobSound {
        mob_id: u64,
        kind_id: u8,
        category: u8,
        pos: petramond_math::world_pos::WorldPos,
    },
    Sound {
        sound_id: u8,
        pos: Option<petramond_math::world_pos::WorldPos>,
    },
    EmitterBurst {
        emitter_id: u8,
        pos: petramond_math::world_pos::WorldPos,
        intensity: f32,
        direction: Option<[f32; 3]>,
        texture: Option<BurstTextureMsg>,
    },
    SpatialSound(SpatialSoundMsg),
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SpatialSoundMsg {
    PlayAt {
        handle: u64,
        sound_id: u8,
        pos: petramond_math::world_pos::WorldPos,
        volume: f32,
        pitch: f32,
    },
    PlayOnMob {
        handle: u64,
        sound_id: u8,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpenScreen {
    Gui {
        kind_key: String,
        anchor: Option<crate::menu::MenuAnchor>,
    },
    Sleep,
}

/// The recipient's own lossy per-tick one-shots: hand jabs, hurt/death,
/// screen opens, and the session's `request_open_gui`/`request_open_sleep`
/// outbox. `broke_block`/`placed_block` carry wire
/// block ids (they pick the client's hand animation + sound mapping).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfEvents {
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
    pub close_document_gui: bool,
    pub toggled_panel: Option<bool>,
    pub used_unpredicted: bool,
    pub used_unpredicted_off: bool,
    pub animator_events: Vec<(crate::player::RigId, u16)>,
    pub client_events: Vec<ClientEventMsg>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEventMsg {
    pub key: String,
    pub data: Vec<u8>,
}

impl SelfEvents {
    pub fn merge_from(&mut self, other: SelfEvents) {
        self.picked_up_item |= other.picked_up_item;
        self.bed_interacted |= other.bed_interacted;
        self.player_damaged |= other.player_damaged;
        self.player_died |= other.player_died;
        self.sleep_ended |= other.sleep_ended;
        self.respawned |= other.respawned;
        if other.open_screen.is_some() {
            self.open_screen = other.open_screen;
        }
        self.close_document_gui |= other.close_document_gui;
        self.toggled_panel = other.toggled_panel.or(self.toggled_panel);
        self.used_unpredicted |= other.used_unpredicted;
        self.used_unpredicted_off |= other.used_unpredicted_off;
        self.animator_events.extend(other.animator_events);
        self.client_events.extend(other.client_events);
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepTally {
    pub sleeping: u16,
    pub connected: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BurstTextureMsg {
    Tile {
        tile: String,
        slice: [f32; 4],
        tint: [u8; 3],
    },
    Block {
        block_id: u16,
        tint: Option<[u8; 3]>,
    },
}
