//! The event bus vocabulary: kinds, payloads, and their support types.

use serde::{Deserialize, Serialize};

use crate::data::MobTagValue;
use crate::ids::{BlockId, ItemId, MobId, PlayerId};

/// A pre-event handler's verdict. The first `Cancel` wins AND ends the
/// dispatch — handlers after it never run on the consumed event. A handler
/// that runs always sees a live event (with any earlier mutations) and may
/// act on it.
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
}

/// Why an entity is taking damage.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Fall,
    /// A player's melee strike; `id` is the attacking session's player id.
    PlayerAttack {
        id: PlayerId,
    },
    /// A mob's melee strike; `key` is the attacking species' key
    /// (`"petramond:owl"`, `"zombies:zombie"`).
    MobAttack {
        key: String,
    },
    /// A mod's [`HostCall::DamagePlayer`]; `mod_id` is the calling mod's
    /// pack id, so handlers can filter by origin.
    ///
    /// [`HostCall::DamagePlayer`]: crate::HostCall::DamagePlayer
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
    /// A mod-defined GUI; `key` is its registered kind key
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
    Ragdoll,
    /// Engine i-frames: a hit that decreases health grants `ticks` of the
    /// victim-global window, and the request is rejected while one is active.
    /// Omit for damage-over-time (burn) that must neither grant nor be
    /// blocked.
    Immunity {
        ticks: u32,
    },
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
                // Mirrors the engine default (10 ticks at 20 TPS).
                MobDamageFeedbackComponent::Immunity { ticks: 10 },
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
    /// PRE — the player's use click as its most PRIMITIVE gesture: what the
    /// crosshair held (a block cell + face, a live mob), nothing more.
    /// Cancel = the attempt was consumed; the block's built-in capability,
    /// the held item's own use, and placement are all skipped. Handlers gate
    /// their own claim by querying the world ([`HostCall::GetBlock`]) and the
    /// acting player's snapshot ([`HostCall::PlayerState`]: held item,
    /// sneak) — attempt context is never pre-interpreted onto the event.
    ///
    /// [`HostCall::GetBlock`]: crate::HostCall::GetBlock
    /// [`HostCall::PlayerState`]: crate::HostCall::PlayerState
    InteractAttempt {
        /// The clicked block cell, if the crosshair held a block.
        block: Option<[i32; 3]>,
        /// The clicked face's normal (back toward the eye; zero when the eye
        /// started inside the cell). `Some` exactly when `block` is.
        face: Option<[i32; 3]>,
        /// The clicked mob's stable session id, if the crosshair held a live
        /// mob (authoritatively validated — a forged, vanished, dead, or
        /// occluded claim never appears here). THE mob address for calls and
        /// cross-tick mod state; species via [`HostCall::MobInfo`].
        ///
        /// [`HostCall::MobInfo`]: crate::HostCall::MobInfo
        mob: Option<u64>,
        /// The interacting session's player id (for per-player calls such as
        /// [`HostCall::MobMount`]).
        ///
        /// [`HostCall::MobMount`]: crate::HostCall::MobMount
        player: PlayerId,
    },
    ItemUsePre {
        item: ItemId,
        target: Option<[i32; 3]>,
    },
    /// A mob damage request that passed the victim's engine-owned immunity gate.
    MobDamagePre {
        /// Stable session id of the struck mob — the address every mob call
        /// takes, and the key for cross-tick mod state on this mob.
        mob_id: u64,
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
    /// POST — a mob died through the damage pipeline. Carries the stable
    /// `id` so a mod releases any per-mob state it keyed by it (despawns and
    /// section unloads fire no event — bound such state maps anyway).
    MobDied {
        /// Stable session id the mob lived under.
        id: u64,
        kind: MobId,
        pos: [f32; 3],
    },
    /// POST — a mob entered the live world (natural, hostile-planner, or a
    /// mod's [`HostCall::SpawnMob`]; save-restores announce as
    /// `section_loaded` instead). Carries the newborn's stable `id`.
    ///
    /// [`HostCall::SpawnMob`]: crate::HostCall::SpawnMob
    MobSpawned {
        /// Stable session id the mob now answers to.
        id: u64,
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
    /// POST — a player left a seat or pose anchor, however it happened (the
    /// engine's sneak gesture, the mount or rider dying, the rider leaving or
    /// turning spectator, or a mod's [`HostCall::MobDismount`]). The mounting
    /// mod uses it to update rider policy (who controls the vehicle).
    /// Mounting has no event: only a mod's own mount/pose call starts one.
    ///
    /// [`HostCall::MobDismount`]: crate::HostCall::MobDismount
    PlayerDismounted {
        player_id: PlayerId,
        /// The mount that was left (the mob may already be gone; an anchor's
        /// furniture may already be air).
        mount: crate::MountTarget,
    },
    /// POST — a key BECAME PRESENT in a live mob's tag map through the ABI
    /// tag surface ([`HostCall::MobTagSet`] inserting a new key). Presence
    /// transitions only: overwriting an existing key's value is silent, and
    /// engine-internal tag churn (health, the confined refresh, spawn
    /// seeding, save restore) and AI-decision writes fire nothing.
    ///
    /// [`HostCall::MobTagSet`]: crate::HostCall::MobTagSet
    MobTagAdded {
        /// The mob's stable session id.
        mob_id: u64,
        /// Its species (session id — bridge with `MobNames`/`ResolveMob`).
        kind: MobId,
        key: String,
        /// The stored value.
        value: MobTagValue,
    },
    /// POST — a present key was DELETED from a live mob's tag map through
    /// the ABI tag surface ([`HostCall::MobTagDelete`]). Same scope rules as
    /// [`MobTagAdded`](Self::MobTagAdded). This is the composable
    /// state-transition hook: e.g. removing a maturity tag is what grows a
    /// juvenile, whoever removes it.
    ///
    /// [`HostCall::MobTagDelete`]: crate::HostCall::MobTagDelete
    MobTagRemoved {
        mob_id: u64,
        kind: MobId,
        key: String,
        /// The value the key held when it was removed.
        value: MobTagValue,
    },
}

impl EventPayload {
    pub fn kind(&self) -> EventKind {
        match self {
            EventPayload::BlockPlacePre { .. } => EventKind::BlockPlacePre,
            EventPayload::BlockBreakPre { .. } => EventKind::BlockBreakPre,
            EventPayload::InteractAttempt { .. } => EventKind::InteractAttempt,
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
        }
    }
}
