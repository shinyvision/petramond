//! Typed event payloads — the mod-facing event taxonomy.
//!
//! Fields mirror today's hardcoded data flows. Pre-event payloads are handed to
//! handlers as `&mut`, but the engine only reads back the fields the taxonomy
//! marks mutable (`MobDamagePre::amount`, `MobDamagePre::feedback`,
//! `PlayerDamagePre::amount`); everything else is observational.

// The payloads are the mod-facing API: the engine constructs them and handlers
// read them; fields no engine handler touches are still part of the surface
// mod closures see through `modding::convert`.
#![allow(dead_code)]

use crate::block::Block;
use crate::chunk::SectionPos;
use crate::facing::Facing;
use crate::item::ItemType;
use crate::mathh::{IVec3, Vec3};
use crate::mob::{Mob, MobDamageFeedback};

/// `block_place_pre` — cancel = placement refused (the click does nothing and the
/// held item is kept).
#[derive(Copy, Clone, Debug)]
pub(crate) struct BlockPlacePre {
    /// The cell the placement targets (the anchor cell for multi-cell models).
    pub pos: IVec3,
    pub block: Block,
    /// The player-derived placement facing; shape paths may orient it further.
    pub facing: Facing,
}

/// `block_break_pre` — cancel = unbreakable (the block stays; the spent mining
/// progress is the cost). Fires only for player mining; sim-destroyed blocks
/// (natural breaks) are not cancellable.
#[derive(Copy, Clone, Debug)]
pub(crate) struct BlockBreakPre {
    pub pos: IVec3,
    pub block: Block,
    /// Whether the held tool harvests drops from this block.
    pub harvested: bool,
}

/// `interact_attempt` — the player's use click as its most PRIMITIVE gesture:
/// what the crosshair held (a block cell + face, a live mob), nothing more.
/// Cancel = the attempt was consumed. Any consumer — mod or engine — gates
/// its own claim by querying the world and the acting player's snapshot
/// (held item, sneak); the attempt itself interprets nothing.
#[derive(Copy, Clone, Debug)]
pub(crate) struct InteractAttempt {
    /// The clicked block cell, if the crosshair held a block.
    pub block: Option<IVec3>,
    /// The clicked face's normal (back toward the eye; zero when the eye
    /// started inside the cell). `Some` exactly when `block` is.
    pub face: Option<IVec3>,
    /// The clicked mob's stable session id, if the crosshair held a live mob
    /// (authoritatively validated before dispatch — a forged, vanished, dead,
    /// or occluded claim never appears here).
    pub mob: Option<u64>,
    /// The interacting session.
    pub player: crate::server::player::PlayerId,
}

/// `item_use_pre` — cancel = the click was consumed (the engine's own use is
/// skipped, but the item still reports as used).
#[derive(Copy, Clone, Debug)]
pub(crate) struct ItemUsePre {
    pub item: ItemType,
    /// The looked-at block, if any.
    pub target: Option<IVec3>,
}

/// `mob_damage_pre` — `amount` and `feedback` are mutable; cancel = no damage.
#[derive(Clone, Debug)]
pub(crate) struct MobDamagePre {
    /// Stable session id of the struck mob (the mob's one mod-facing address).
    pub mob_id: u64,
    pub kind: Mob,
    pub amount: f32,
    pub source: DamageSource,
    /// Optional world-space origin for attack knockback or spatial feedback.
    pub origin: Option<Vec3>,
    /// Mutable default feedback controls for damage that survives this hook.
    pub feedback: MobDamageFeedback,
}

/// `player_damage_pre` — `amount` is mutable; cancel = no damage. Non-positive
/// or engine-immunity-blocked damage is a non-event and never dispatches.
#[derive(Copy, Clone, Debug)]
pub(crate) struct PlayerDamagePre {
    pub amount: i32,
    pub source: DamageSource,
    /// Optional world-space origin for attack knockback or spatial feedback.
    pub origin: Option<Vec3>,
}

/// Why an entity is taking damage. Knockback is only a default consequence for
/// explicit attack sources; `origin` on a payload is spatial context, not proof
/// that knockback should happen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DamageSource {
    Fall,
    /// A player's melee strike; carries the attacking session.
    PlayerAttack(crate::server::player::PlayerId),
    /// A mob's melee strike; carries the attacking species AND the attacker's
    /// stable mob id — the source names WHO caused the damage, and for a mob
    /// that identity is the instance (the struck target's retaliation memory
    /// needs the biter, not just its species).
    MobAttack {
        kind: Mob,
        id: u64,
    },
    /// A mod's `DamagePlayer` HostCall; carries the mod's pack id
    /// (interned for the process lifetime — see `modding::host`), so handlers
    /// can filter by origin.
    Mod(&'static str),
}

impl DamageSource {
    #[inline]
    pub(crate) fn is_attack(self) -> bool {
        matches!(self, Self::PlayerAttack(_) | Self::MobAttack { .. })
    }

    /// The attacking ENTITY this source names, when it names one — the value
    /// recorded as a struck mob's retaliation memory. Fall and mod damage have
    /// no attacker identity.
    #[inline]
    pub(crate) fn attacker(self) -> Option<crate::mob::EntityRef> {
        match self {
            Self::PlayerAttack(pid) => Some(crate::mob::EntityRef::Player(pid)),
            Self::MobAttack { id, .. } => Some(crate::mob::EntityRef::Mob(id)),
            Self::Fall | Self::Mod(_) => None,
        }
    }
}

/// An engine action a mod HostCall queued from inside a guest dispatch, where
/// the event bus is already borrowed and cannot be re-entered. `Game` drains
/// these at defined per-tick points (after every systems batch and before each
/// post-event drain) and routes each through the same funnel the engine's own
/// code uses, so global engine immunity and registered pre handlers still apply.
#[derive(Clone, Debug)]
pub(crate) enum ModAction {
    /// `Game::damage_player(amount, DamageSource::Mod(mod_id))`.
    DamagePlayer { amount: i32, mod_id: &'static str },
    /// The mob-damage pipeline (`mob_damage_pre` → `Mobs::damage_mob` → death loot).
    /// Carries the STABLE mob id; the drain resolves it to a live index only
    /// then (earlier actions this drain may have shifted storage). Mod damage
    /// is not an attack, so it does not receive default knockback.
    DamageMob {
        mob_id: u64,
        amount: f32,
        mod_id: &'static str,
        origin: Option<Vec3>,
        /// The request's composed damage pipeline; `None` = the species'
        /// resolved `damage_feedback`. No `Immunity` component = DoT (burn):
        /// neither blocked by an active i-frame window nor granting one.
        feedback: Option<crate::mob::MobDamageFeedback>,
    },
    /// A mod's `GuiOpen` HostCall: request the app shell open this mod GUI
    /// (honoured only from gameplay, like a block-interact open request).
    OpenGui { kind: crate::gui::GuiKind },
    /// A mod's `GuiClose` HostCall: close the open mod GUI, if one is open.
    CloseGui,
    /// A mod's `ChatSend` HostCall: deliver one authored chat line on the next
    /// pump. `None` targets = all connected sessions; `Some(ids)` = those
    /// player ids only (unknown / left ids ignored).
    ChatSend {
        text: String,
        targets: Option<Vec<u8>>,
    },
}

/// Observational events, queued at their site and drained FIFO at the post-queue
/// drain points (each tick-stage boundary) within the same tick.
/// (`Copy` was dropped when the tag events gained String keys — a Rust-trait
/// change only, like `ContainerKind`'s.)
#[derive(Clone, Debug)]
pub(crate) enum PostEvent {
    BlockPlaced {
        pos: IVec3,
        block: Block,
    },
    BlockBroken {
        pos: IVec3,
        block: Block,
        harvested: bool,
        /// True when the simulation destroyed the block (support loss, washed
        /// away) rather than the player mining it.
        natural: bool,
    },
    ItemUsed {
        item: ItemType,
    },
    MobDied {
        /// Stable session id the mob lived under — how a mod releases per-mob
        /// state keyed by it.
        id: u64,
        kind: Mob,
        pos: Vec3,
    },
    MobSpawned {
        /// The newborn's stable session id.
        id: u64,
        kind: Mob,
        pos: Vec3,
    },
    PlayerDamaged {
        amount: i32,
        new_health: i32,
    },
    /// Health crossed >0 → 0. NO default consequence — a mod (or future core
    /// content) decides what death means.
    PlayerDied,
    /// A container GUI session began. `kind` is the session's registered
    /// `GuiKind` — engine containers (the inventory's own recipe browser
    /// included) and mod GUIs speak the one kind registry; the ABI mirror
    /// carries the kind's key string.
    ContainerOpened {
        kind: crate::gui::GuiKind,
        pos: Option<IVec3>,
    },
    ContainerClosed {
        kind: crate::gui::GuiKind,
        pos: Option<IVec3>,
    },
    SectionGenerated {
        pos: SectionPos,
    },
    SectionLoaded {
        pos: SectionPos,
    },
    /// A player left a mob seat — however it happened (sneak gesture, the
    /// mount died/despawned/unloaded, the rider died, left, or turned
    /// spectator, or a mod's `MobDismount`). The mounting mod uses this to
    /// clean up its rider policy (who controls the vehicle); mounting itself
    /// has no event — only a mod's own `MobMount` call starts a ride.
    PlayerDismounted {
        player: crate::server::player::PlayerId,
        mob_id: u64,
    },
    /// A key BECAME PRESENT in a live mob's tag map through the ABI tag
    /// surface (`MobTagSet` inserting a new key). Presence transitions only —
    /// value overwrites, engine-internal tag churn, spawn seeding, save
    /// restore, and AI-decision writes all fire nothing.
    MobTagAdded {
        id: u64,
        kind: Mob,
        key: String,
        value: crate::mob::MobTagValue,
    },
    /// A present key was DELETED from a live mob's tag map through the ABI
    /// tag surface (`MobTagDelete`). `value` is what it held.
    MobTagRemoved {
        id: u64,
        kind: Mob,
        key: String,
        value: crate::mob::MobTagValue,
    },
}

/// Registration key for post handlers; one bit per kind gates enqueueing so an
/// unheard event costs nothing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PostEventKind {
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

impl PostEventKind {
    pub(crate) const COUNT: usize = 14;
}

impl PostEvent {
    pub(crate) fn kind(&self) -> PostEventKind {
        match self {
            PostEvent::BlockPlaced { .. } => PostEventKind::BlockPlaced,
            PostEvent::BlockBroken { .. } => PostEventKind::BlockBroken,
            PostEvent::ItemUsed { .. } => PostEventKind::ItemUsed,
            PostEvent::MobDied { .. } => PostEventKind::MobDied,
            PostEvent::MobSpawned { .. } => PostEventKind::MobSpawned,
            PostEvent::PlayerDamaged { .. } => PostEventKind::PlayerDamaged,
            PostEvent::PlayerDied => PostEventKind::PlayerDied,
            PostEvent::ContainerOpened { .. } => PostEventKind::ContainerOpened,
            PostEvent::ContainerClosed { .. } => PostEventKind::ContainerClosed,
            PostEvent::SectionGenerated { .. } => PostEventKind::SectionGenerated,
            PostEvent::SectionLoaded { .. } => PostEventKind::SectionLoaded,
            PostEvent::PlayerDismounted { .. } => PostEventKind::PlayerDismounted,
            PostEvent::MobTagAdded { .. } => PostEventKind::MobTagAdded,
            PostEvent::MobTagRemoved { .. } => PostEventKind::MobTagRemoved,
        }
    }
}
