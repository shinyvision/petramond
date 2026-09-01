//! The event bus vocabulary: kinds, payloads, and their support types.

use serde::{Deserialize, Serialize};

use crate::data::{ItemStackData, MobTagValue};
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
    ItemPickedUp,
    ItemObtained,
    MobDamaged,
    Interacted,
    /// Every mod-authored event, whoever emitted it
    /// ([`HostCall::EmitEvent`]). Handlers filter by the payload's key; there
    /// is no per-key registration.
    ///
    /// [`HostCall::EmitEvent`]: crate::HostCall::EmitEvent
    ModEvent,
    /// A use gesture NOTHING claimed — the fall-through, fired once after the
    /// whole interact chain passed (including on a click at nothing at all).
    ///
    /// A real PRESS only: the held-button repeat never offers it, because a
    /// continuous use is something the player asked for, and because a client
    /// predicts the press and nothing else.
    ///
    /// This is where a CONTINUOUS use lives: call [`HostCall::HoldUse`] to take
    /// the press and keep it until the button comes up. Cancel only ends the
    /// dispatch for later handlers — by the time this fires the chain has
    /// already passed, so nothing happened to the world and no hand jabs.
    ///
    /// [`HostCall::HoldUse`]: crate::HostCall::HoldUse
    UseUnclaimed,
    /// PRE — the player's primary-button press as its most primitive
    /// gesture: what the crosshair held (a block, a live mob, another
    /// player) and who pressed. Fires for EVERY accepted press, a press at
    /// nothing included; a body whose attack is denied or still on cooldown
    /// never dispatches one. Cancel = the press is yours: the engine's own
    /// melee (the crosshair hit, the air punch) stands down for it, the
    /// hand still swings, the attack cooldown still arms, and landing the
    /// hit — when, on whom, how hard — is the claimant's to do through
    /// [`HostCall::DamageMob`] / [`HostCall::DamagePlayer`] naming the
    /// presser as the attacker. Mining is the held button on a block, not
    /// the press, so it runs whoever takes the press.
    ///
    /// [`HostCall::DamageMob`]: crate::HostCall::DamageMob
    /// [`HostCall::DamagePlayer`]: crate::HostCall::DamagePlayer
    AttackAttempt,
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

/// Which container GUI opened/closed, named by its registered kind key —
/// `"petramond:chest"`, `"petramond:furnace"`, `"kitchen:oven"`.
///
/// Engine and pack containers speak ONE vocabulary here, the way
/// [`DamageSource::MobAttack`] speaks species keys. There are deliberately no
/// engine-named variants: a pack container would be second-class beside them,
/// and every engine container added later would be a wire break.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContainerKind {
    pub key: String,
}

impl ContainerKind {
    pub fn new(key: impl Into<String>) -> Self {
        ContainerKind { key: key.into() }
    }

    /// Whether this is the container registered under `key`.
    pub fn is(&self, key: &str) -> bool {
        self.key == key
    }
}

/// WHICH use an [`EventPayload::ItemUsed`] is reporting. A handler that cares
/// about one path must check this: the event fires from four sites, and two
/// of them are a mod claiming the click rather than the engine acting.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUseEvent {
    /// A held-eat ran to completion: the portion is off the stack and the
    /// row's effects have landed. Where a pack hangs what the food LEAVES
    /// BEHIND (a stew's bowl) — see [`EventPayload::ItemUsed`].
    Eaten,
    /// The item's row-declared `use` handler ran (a bucket filled or poured;
    /// its held stack is already the counterpart item).
    Handler,
    /// A mod's `item_use_pre` claimed the click and the engine did nothing
    /// further. Reported so no use goes unaccounted for, but the mod that
    /// claimed it already knows what it did.
    Claimed,
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
/// [`MobDamagePre::feedback`], [`PlayerDamagePre::amount`],
/// [`BlockBreakPre::drops`]) — everything else is observational.
///
/// [`MobDamagePre::amount`]: EventPayload::MobDamagePre
/// [`PlayerDamagePre::amount`]: EventPayload::PlayerDamagePre
/// [`MobDamagePre::feedback`]: EventPayload::MobDamagePre
/// [`BlockBreakPre::drops`]: EventPayload::BlockBreakPre
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EventPayload {
    BlockPlacePre {
        pos: [i32; 3],
        block: BlockId,
        facing: Facing,
    },
    /// PRE — a player break that is about to clear the cell. Cancel =
    /// unbreakable (the block stays). Fires for player mining only;
    /// sim-destroyed blocks (natural breaks) never dispatch it.
    BlockBreakPre {
        pos: [i32; 3],
        block: BlockId,
        /// Whether the held tool passes the block's harvest gate (drops
        /// would spawn). Observational — a drops override is honored
        /// regardless, so a handler that only wants harvested breaks gates
        /// on this itself.
        harvested: bool,
        /// The breaking session's player id (for per-player calls such as
        /// [`HostCall::PlayerHeld`]).
        ///
        /// [`HostCall::PlayerHeld`]: crate::HostCall::PlayerHeld
        player: PlayerId,
        /// Mutable: written back by the engine after the dispatch. `None` =
        /// the engine's own drop tables roll as usual; `Some(stacks)` = the
        /// break drops EXACTLY these stacks instead (empty = nothing), the
        /// engine spawns them verbatim — instance data included — and a
        /// stack naming the broken block's own item still picks up the
        /// cell's carried data. Later handlers in the chain see an earlier
        /// handler's override and may leave or replace it.
        drops: Option<Vec<ItemStackData>>,
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
    /// POST — a use of `item` by `player` was consumed. `kind` says WHICH
    /// use (see [`ItemUseEvent`]); without it the four paths are
    /// indistinguishable and a handler cannot react to eating in particular.
    ///
    /// `ItemUseEvent::Eaten` is the seam for whatever a food LEAVES BEHIND:
    /// the portion is already off the stack and its effects have landed, so a
    /// pack gives back the bowl/bottle here with `GiveItemTo` (an EXPLICIT
    /// player — the payload names who ate). That is deliberately pack policy:
    /// a container is what a recipe put the food in, not something the engine
    /// should have vocabulary for.
    ItemUsed {
        player: PlayerId,
        item: ItemId,
        kind: ItemUseEvent,
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
    /// POST — a player vacuumed dropped-item entities off the ground. One
    /// event per collected STACK, so a tick that sweeps three drops fires
    /// three times. This is the "it came off the floor" signal (magnets,
    /// collection quests, pickup effects); for "the player now has one of
    /// these at all", listen on [`ItemObtained`](Self::ItemObtained) instead.
    ItemPickedUp {
        player: PlayerId,
        item: ItemId,
        count: u8,
        /// Where the collector's body was — the drop is already gone.
        pos: [f32; 3],
    },
    /// POST — an item kind entered a player's inventory for the FIRST time
    /// ever, from ANY source: a pickup, a craft, a furnace output, a chest
    /// withdrawal, a mod's [`HostCall::GiveItem`]. The engine owns the
    /// per-player "ever held" set that makes this a once-per-kind transition
    /// (it persists with the player), so a handler needs no memory of its own.
    ///
    /// This is the progression signal: the engine's own recipe unlocking
    /// listens on it (see the crafting docs), and a mod that wants "the first
    /// time the player holds X" should too rather than polling an inventory.
    ///
    /// [`HostCall::GiveItem`]: crate::HostCall::GiveItem
    ItemObtained {
        player: PlayerId,
        item: ItemId,
    },
    /// POST — damage LANDED on a mob (it survived `mob_damage_pre`, the
    /// i-frame gate, and had at least one feedback component). `amount` is
    /// the post-mutation value the pipeline actually applied. A killing blow
    /// fires this AND [`MobDied`](Self::MobDied).
    MobDamaged {
        mob_id: u64,
        kind: MobId,
        amount: f32,
        source: DamageSource,
        /// Whether this hit was the killing one.
        killed: bool,
    },
    /// POST — a use click RESOLVED, whoever consumed it. Fires for every
    /// attempt that named a block cell or a mob, exactly once, after the
    /// consumer chain ran — the observational twin of the cancellable
    /// [`InteractAttempt`](Self::InteractAttempt) (which a mod earlier in the
    /// chain can end before later handlers ever see it). `consumed` says
    /// whether anything claimed it.
    ///
    /// Observe here; CLAIM on `interact_attempt`. A HELD use button
    /// dispatches a fresh attempt every few ticks, exactly like a click, so
    /// this fires for those repeats too — a handler must be happy to run
    /// several times a second while the button is down.
    Interacted {
        block: Option<[i32; 3]>,
        face: Option<[i32; 3]>,
        mob: Option<u64>,
        player: PlayerId,
        consumed: bool,
    },
    /// POST — a mod emitted its own event ([`HostCall::EmitEvent`]). `key` is
    /// namespaced to the EMITTING mod (`"farming:harvest_complete"`) and
    /// `data` is that mod's own opaque payload; every registered handler sees
    /// every mod event, so filter on `key` first.
    ///
    /// [`HostCall::EmitEvent`]: crate::HostCall::EmitEvent
    ModEvent {
        key: String,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// A use gesture the whole interact chain passed on ([`EventKind::UseUnclaimed`]).
    ///
    /// The same context [`InteractAttempt`](Self::InteractAttempt) carries, and
    /// every field may be absent: a click at empty air is exactly the case this
    /// exists for. [`HostCall::HoldUse`] is what takes the press.
    ///
    /// [`HostCall::HoldUse`]: crate::HostCall::HoldUse
    UseUnclaimed {
        block: Option<[i32; 3]>,
        face: Option<[i32; 3]>,
        mob: Option<u64>,
        player: PlayerId,
    },
    /// PRE — one primary-button press ([`EventKind::AttackAttempt`]): what
    /// the crosshair held and who pressed, nothing pre-interpreted. `mob`
    /// and `target` are authority-validated (a forged, vanished, dead,
    /// occluded or out-of-reach claim never appears here); a press at
    /// nothing carries all three `None`. Cancel = claimed — see the kind.
    AttackAttempt {
        /// The block cell under the crosshair, if any (a press here is
        /// mining's; the engine's melee passes on it).
        block: Option<[i32; 3]>,
        /// The clicked face's normal (back toward the eye). `Some` exactly
        /// when `block` is.
        face: Option<[i32; 3]>,
        /// The live mob under the crosshair, by stable id.
        mob: Option<u64>,
        /// The OTHER player under the crosshair (alive, in reach).
        target: Option<PlayerId>,
        /// The pressing session.
        player: PlayerId,
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
        }
    }
}
