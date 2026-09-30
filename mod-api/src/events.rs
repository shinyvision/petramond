use serde::{Deserialize, Serialize};

use crate::data::{EntityRef, ItemStackData, MobTagValue};
use crate::ids::{BlockId, ItemId, MobId, PlayerId};

mod filter;

pub use filter::*;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Cancel,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    BlockPlacePre,
    BlockBreakPre,
    InteractAttempt,
    ItemUsePre,
    MobDamagePre,
    PlayerDamagePre,
    BlockPlaced,
    BlockBroken,
    ItemUsed,
    MobDied,
    MobSpawned,
    PlayerDamaged,
    PlayerDied,
    ContainerOpened,
    ContainerClosed,
    SectionGenerated,
    SectionLoaded,
    PlayerDismounted,
    MobTagAdded,
    MobTagRemoved,
    ItemPickedUp,
    ItemObtained,
    MobDamaged,
    Interacted,
    ModEvent,
    UseUnclaimed,
    /// PRE — primary-button press, most primitive: what the crosshair held (block, mob, player)
    /// and who pressed.
    /// Fires for every accepted press, even at nothing. Denied or cooldown-blocked attacks don't
    /// dispatch one.
    /// Cancel means the press is yours: the engine's own melee stands down, but the hand swing and
    /// cooldown still happen.
    /// Landing the hit is on you, via [`EntityCall::DamageMob`] / [`PlayerCall::DamagePlayer`]
    /// naming the presser as attacker.
    /// Mining is the held button on a block, not the press, so it runs whoever takes the press.
    ///
    /// [`EntityCall::DamageMob`]: crate::EntityCall::DamageMob
    /// [`PlayerCall::DamagePlayer`]: crate::PlayerCall::DamagePlayer
    AttackAttempt,
    ProjectileHit,
    ActorActed,
    SchematicChosen,
    SchematicPositioned,
    CellsEditPre,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum ProjectileFate {
    Consume,
    Lodge,
    Drop,
    /// Keep the same projectile in flight with a new velocity, in blocks per second.
    Deflect { vel: [f32; 3] },
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum ProjectileTarget {
    Mob(u64),
    Player(PlayerId),
    Block { pos: [i32; 3], face: [i32; 3] },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Fall,
    PlayerAttack { id: PlayerId },
    MobAttack { key: String },
    Mod { mod_id: String },
    FluidContact { block: BlockId },
    Condition { condition: crate::ConditionId },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContainerKind {
    pub key: String,
}

impl ContainerKind {
    pub fn new(key: impl Into<String>) -> Self {
        ContainerKind { key: key.into() }
    }

    pub fn is(&self, key: &str) -> bool {
        self.key == key
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUseEvent {
    Eaten,
    Handler,
    Claimed,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Facing {
    North,
    South,
    West,
    East,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobDamageFeedback {
    pub components: Vec<MobDamageFeedbackComponent>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum MobDamageFeedbackComponent {
    DecreaseHealth,
    Flash {
        duration: f32,
    },
    Knockback {
        scale: f32,
        duration: f32,
    },
    Sound {
        category: MobDamageSound,
    },
    Ragdoll {
        joints: RagdollJoints,
        /// Scales the added launch and scatter (0–8); inherited velocity and gravity are unchanged.
        impulse_scale: f32,
    },
    Immunity {
        ticks: u32,
    },
}

/// Physical links between ragdoll bodies; welded model details remain attached in either mode.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum RagdollJoints {
    Connected,
    Detached,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum MobDamageSound {
    Hurt,
    Death,
}

impl Default for MobDamageFeedback {
    fn default() -> Self {
        Self {
            components: vec![
                MobDamageFeedbackComponent::DecreaseHealth,
                MobDamageFeedbackComponent::Flash { duration: 0.3 },
                MobDamageFeedbackComponent::Knockback {
                    scale: 1.0,
                    duration: 0.3,
                },
                MobDamageFeedbackComponent::Sound {
                    category: MobDamageSound::Hurt,
                },
                MobDamageFeedbackComponent::Sound {
                    category: MobDamageSound::Death,
                },
                MobDamageFeedbackComponent::Ragdoll {
                    joints: RagdollJoints::Connected,
                    impulse_scale: 1.0,
                },
                MobDamageFeedbackComponent::Immunity { ticks: 10 },
            ],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EventPayload {
    BlockPlacePre {
        pos: [i32; 3],
        block: BlockId,
        facing: Facing,
        actor: EntityRef,
    },
    BlockBreakPre {
        pos: [i32; 3],
        block: BlockId,
        harvested: bool,
        actor: EntityRef,
        drops: Option<Vec<ItemStackData>>,
    },
    InteractAttempt {
        block: Option<[i32; 3]>,
        face: Option<[i32; 3]>,
        mob: Option<u64>,
        player: PlayerId,
    },
    ItemUsePre {
        item: ItemId,
        target: Option<[i32; 3]>,
    },
    MobDamagePre {
        mob_id: u64,
        kind: MobId,
        amount: f32,
        source: DamageSource,
        origin: Option<[f64; 3]>,
        feedback: MobDamageFeedback,
    },
    PlayerDamagePre {
        amount: i32,
        source: DamageSource,
        origin: Option<[f64; 3]>,
    },
    BlockPlaced {
        pos: [i32; 3],
        block: BlockId,
    },
    BlockBroken {
        pos: [i32; 3],
        block: BlockId,
        harvested: bool,
        natural: bool,
    },
    ItemUsed {
        player: PlayerId,
        item: ItemId,
        kind: ItemUseEvent,
    },
    MobDied {
        id: u64,
        kind: MobId,
        pos: [f64; 3],
    },
    MobSpawned {
        id: u64,
        kind: MobId,
        pos: [f64; 3],
    },
    PlayerDamaged {
        amount: i32,
        new_health: i32,
    },
    PlayerDied,
    ContainerOpened {
        kind: ContainerKind,
        at: Option<crate::ContainerAddress>,
    },
    ContainerClosed {
        kind: ContainerKind,
        at: Option<crate::ContainerAddress>,
    },
    SectionGenerated {
        pos: [i32; 3],
    },
    SectionLoaded {
        pos: [i32; 3],
    },
    PlayerDismounted {
        player_id: PlayerId,
        mount: crate::MountTarget,
    },
    MobTagAdded {
        mob_id: u64,
        kind: MobId,
        key: String,
        value: MobTagValue,
    },
    MobTagRemoved {
        mob_id: u64,
        kind: MobId,
        key: String,
        value: MobTagValue,
    },
    ItemPickedUp {
        player: PlayerId,
        item: ItemId,
        count: u8,
        pos: [f64; 3],
    },
    ItemObtained {
        player: PlayerId,
        item: ItemId,
    },
    MobDamaged {
        mob_id: u64,
        kind: MobId,
        amount: f32,
        source: DamageSource,
        killed: bool,
    },
    Interacted {
        block: Option<[i32; 3]>,
        face: Option<[i32; 3]>,
        mob: Option<u64>,
        player: PlayerId,
        consumed: bool,
    },
    ModEvent {
        key: String,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    UseUnclaimed {
        block: Option<[i32; 3]>,
        face: Option<[i32; 3]>,
        mob: Option<u64>,
        player: PlayerId,
    },
    AttackAttempt {
        block: Option<[i32; 3]>,
        face: Option<[i32; 3]>,
        mob: Option<u64>,
        target: Option<PlayerId>,
        player: PlayerId,
    },
    ProjectileHit {
        entity: u64,
        item: ItemId,
        target: ProjectileTarget,
        pos: [f64; 3],
        vel: [f32; 3],
        fate: ProjectileFate,
    },
    ActorActed {
        actor: EntityRef,
        pos: [i32; 3],
        action: crate::data::ActorAction,
        refusal: Option<crate::data::ActionRefusal>,
    },
    SchematicChosen {
        player: PlayerId,
        tag: String,
        asset: crate::data::SchematicId,
    },
    SchematicPositioned {
        player: PlayerId,
        tag: String,
        asset: crate::data::SchematicId,
        origin: [i32; 3],
        turns: u8,
    },
    CellsEditPre {
        min: [i32; 3],
        max: [i32; 3],
        cells: u64,
        actor: EntityRef,
    },
}

impl EventPayload {
    pub fn kind(&self) -> EventKind {
        match self {
            EventPayload::BlockPlacePre { .. } => EventKind::BlockPlacePre,
            EventPayload::BlockBreakPre { .. } => EventKind::BlockBreakPre,
            EventPayload::InteractAttempt { .. } => EventKind::InteractAttempt,
            EventPayload::UseUnclaimed { .. } => EventKind::UseUnclaimed,
            EventPayload::AttackAttempt { .. } => EventKind::AttackAttempt,
            EventPayload::ItemUsePre { .. } => EventKind::ItemUsePre,
            EventPayload::MobDamagePre { .. } => EventKind::MobDamagePre,
            EventPayload::PlayerDamagePre { .. } => EventKind::PlayerDamagePre,
            EventPayload::BlockPlaced { .. } => EventKind::BlockPlaced,
            EventPayload::BlockBroken { .. } => EventKind::BlockBroken,
            EventPayload::ItemUsed { .. } => EventKind::ItemUsed,
            EventPayload::MobDied { .. } => EventKind::MobDied,
            EventPayload::MobSpawned { .. } => EventKind::MobSpawned,
            EventPayload::PlayerDamaged { .. } => EventKind::PlayerDamaged,
            EventPayload::PlayerDied => EventKind::PlayerDied,
            EventPayload::ContainerOpened { .. } => EventKind::ContainerOpened,
            EventPayload::ContainerClosed { .. } => EventKind::ContainerClosed,
            EventPayload::SectionGenerated { .. } => EventKind::SectionGenerated,
            EventPayload::SectionLoaded { .. } => EventKind::SectionLoaded,
            EventPayload::PlayerDismounted { .. } => EventKind::PlayerDismounted,
            EventPayload::MobTagAdded { .. } => EventKind::MobTagAdded,
            EventPayload::MobTagRemoved { .. } => EventKind::MobTagRemoved,
            EventPayload::ItemPickedUp { .. } => EventKind::ItemPickedUp,
            EventPayload::ItemObtained { .. } => EventKind::ItemObtained,
            EventPayload::MobDamaged { .. } => EventKind::MobDamaged,
            EventPayload::Interacted { .. } => EventKind::Interacted,
            EventPayload::ModEvent { .. } => EventKind::ModEvent,
            EventPayload::ProjectileHit { .. } => EventKind::ProjectileHit,
            EventPayload::ActorActed { .. } => EventKind::ActorActed,
            EventPayload::SchematicChosen { .. } => EventKind::SchematicChosen,
            EventPayload::SchematicPositioned { .. } => EventKind::SchematicPositioned,
            EventPayload::CellsEditPre { .. } => EventKind::CellsEditPre,
        }
    }
}
