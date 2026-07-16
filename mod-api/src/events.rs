//! The event bus vocabulary: kinds, payloads, and their support types.

use serde::{Deserialize, Serialize};

use crate::ids::{BlockId, ItemId, MobId};

/// A pre-event handler's verdict. The first `Cancel` wins; later handlers still
/// observe the (possibly mutated) payload.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Cancel,
}

/// Every dispatchable event, pre and post.
/// Registration key for [`HostCall::RegisterEventHandler`].
///
/// [`HostCall::RegisterEventHandler`]: crate::HostCall::RegisterEventHandler
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    BlockPlacePre,
    BlockBreakPre,
    BlockInteract,
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
    MobInteract,
    PlayerDismounted,
}

/// Why an entity is taking damage.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Fall,
    /// A player's melee strike; `id` is the attacking session's player id.
    PlayerAttack {
        id: u8,
    },
    /// A mob's melee strike; `key` is the attacking species' key
    /// (`"petramond:owl"`, `"zombies:zombie"`).
    MobAttack {
        key: String,
    },
    /// A mod's [`HostCall::DamagePlayer`] / [`HostCall::KillPlayer`]; `mod_id`
    /// is the calling mod's pack id, so handlers can filter by origin.
    ///
    /// [`HostCall::DamagePlayer`]: crate::HostCall::DamagePlayer
    /// [`HostCall::KillPlayer`]: crate::HostCall::KillPlayer
    Mod {
        mod_id: String,
    },
}

/// Which container GUI opened/closed.
/// (`Copy` was dropped when `Mod` gained its String payload — a Rust-trait
/// change, not a wire change.)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Inventory,
    CraftingTable,
    Furnace,
    Chest,
    FurnitureWorkbench,
    /// A mod-defined GUI (Phase 5); `key` is its registered kind key
    /// (`"wheel:wheel"`).
    Mod {
        key: String,
    },
}

/// Player-derived placement facing.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Facing {
    North,
    South,
    West,
    East,
}

/// Default feedback controls for mob damage that survived `mob_damage_pre`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobDamageFeedback {
    pub components: Vec<MobDamageFeedbackComponent>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum MobDamageFeedbackComponent {
    DecreaseHealth,
    Flash { duration: f32 },
    Knockback { scale: f32, duration: f32 },
    Sound { category: MobDamageSound },
    Ragdoll,
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
                MobDamageFeedbackComponent::Ragdoll,
            ],
        }
    }
}

/// One event's data, mirrored from the engine payloads.
/// Pre events hand the payload to the guest `&mut`; the engine reads
/// back ONLY the fields the taxonomy marks mutable ([`MobDamagePre::amount`],
/// [`MobDamagePre::feedback`], [`PlayerDamagePre::amount`]) — everything else
/// is observational.
///
/// [`MobDamagePre::amount`]: EventPayload::MobDamagePre
/// [`PlayerDamagePre::amount`]: EventPayload::PlayerDamagePre
/// [`MobDamagePre::feedback`]: EventPayload::MobDamagePre
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EventPayload {
    BlockPlacePre {
        pos: [i32; 3],
        block: BlockId,
        facing: Facing,
    },
    BlockBreakPre {
        pos: [i32; 3],
        block: BlockId,
        harvested: bool,
    },
    BlockInteract {
        pos: [i32; 3],
        block: BlockId,
    },
    ItemUsePre {
        item: ItemId,
        target: Option<[i32; 3]>,
    },
    /// A mob damage request that passed the victim's engine-owned immunity gate.
    MobDamagePre {
        /// Index into the live mob set, valid this tick only.
        mob: u32,
        kind: MobId,
        /// Mutable: written back by the engine after the dispatch.
        amount: f32,
        source: DamageSource,
        /// Optional world-space origin for attack knockback or spatial feedback.
        origin: Option<[f32; 3]>,
        /// Mutable: written back by the engine after the dispatch.
        feedback: MobDamageFeedback,
    },
    /// A player damage request that passed the victim's engine-owned immunity gate.
    PlayerDamagePre {
        /// Mutable: written back by the engine after the dispatch.
        amount: i32,
        source: DamageSource,
        /// Optional world-space origin for attack knockback or spatial feedback.
        origin: Option<[f32; 3]>,
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
        item: ItemId,
    },
    MobDied {
        kind: MobId,
        pos: [f32; 3],
    },
    MobSpawned {
        kind: MobId,
        pos: [f32; 3],
    },
    PlayerDamaged {
        amount: i32,
        new_health: i32,
    },
    PlayerDied,
    ContainerOpened {
        kind: ContainerKind,
        pos: Option<[i32; 3]>,
    },
    ContainerClosed {
        kind: ContainerKind,
        pos: Option<[i32; 3]>,
    },
    SectionGenerated {
        /// Section coordinates (16³ units).
        pos: [i32; 3],
    },
    SectionLoaded {
        pos: [i32; 3],
    },
    /// PRE — a use click whose crosshair target was a live mob, dispatched
    /// before any engine mob use (shears). Cancel = the click was consumed:
    /// this is how a mod makes a mob interactable (mounting a vehicle,
    /// trading). Carries both addresses: the tick-local `mob` index for
    /// immediate calls and the stable `id` for cross-tick mod state.
    MobInteract {
        /// Index into the live mob set, valid this tick only.
        mob: u32,
        /// Stable mob session id.
        id: u64,
        /// Species key (`"vehicles:boat"`) — self-describing, no resolver
        /// needed.
        key: String,
        /// The interacting session's player id.
        player_id: u8,
    },
    /// POST — a player left a mob seat, however it happened (the engine's
    /// sneak gesture, the mount or rider dying, the rider leaving or turning
    /// spectator, or a mod's [`HostCall::MobDismount`]). The mounting mod
    /// uses it to update rider policy (who controls the vehicle). Mounting
    /// has no event: only a mod's own [`HostCall::MobMount`] starts a ride.
    ///
    /// [`HostCall::MobMount`]: crate::HostCall::MobMount
    /// [`HostCall::MobDismount`]: crate::HostCall::MobDismount
    PlayerDismounted {
        player_id: u8,
        /// Stable id of the mob that was ridden (it may already be gone).
        mob_id: u64,
    },
}

impl EventPayload {
    pub fn kind(&self) -> EventKind {
        match self {
            EventPayload::BlockPlacePre { .. } => EventKind::BlockPlacePre,
            EventPayload::BlockBreakPre { .. } => EventKind::BlockBreakPre,
            EventPayload::BlockInteract { .. } => EventKind::BlockInteract,
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
            EventPayload::MobInteract { .. } => EventKind::MobInteract,
            EventPayload::PlayerDismounted { .. } => EventKind::PlayerDismounted,
        }
    }
}
