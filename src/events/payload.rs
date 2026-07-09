//! Typed event payloads — the Phase 1 taxonomy from WIKI/modding.md.
//!
//! Fields mirror today's hardcoded data flows. Pre-event payloads are handed to
//! handlers as `&mut`, but the engine only reads back the fields the taxonomy
//! marks mutable (`MobDamagePre::amount`, `MobDamagePre::feedback`,
//! `PlayerDamagePre::amount`); everything else is observational in Phase 1.

// The payloads are the mod-facing API: the engine constructs them and only
// handlers read them, and no engine handlers exist until Phase 2 — so dead-code
// analysis cannot see the reads yet.
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
/// (natural breaks) are not cancellable in Phase 1.
#[derive(Copy, Clone, Debug)]
pub(crate) struct BlockBreakPre {
    pub pos: IVec3,
    pub block: Block,
    /// Whether the held tool harvests drops from this block.
    pub harvested: bool,
}

/// `block_interact` — cancel = the click was consumed (this is how mod blocks
/// will open their own GUIs); the block's built-in capability is skipped.
#[derive(Copy, Clone, Debug)]
pub(crate) struct BlockInteract {
    pub pos: IVec3,
    pub block: Block,
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
    /// Index into the live mob set, valid this tick only.
    pub mob: usize,
    pub kind: Mob,
    pub amount: f32,
    pub source: DamageSource,
    /// Optional world-space origin for attack knockback or spatial feedback.
    pub origin: Option<Vec3>,
    /// Mutable default feedback controls for damage that survives this hook.
    pub feedback: MobDamageFeedback,
}

/// `player_damage_pre` — `amount` is mutable; cancel = no damage (i-frames live
/// here). Non-positive incoming damage is a non-event and never dispatches.
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
    /// A mob's melee strike; carries the attacking species.
    MobAttack(Mob),
    /// A mod's `DamagePlayer`/`KillPlayer` HostCall; carries the mod's pack id
    /// (interned for the process lifetime — see `modding::host`), so handlers
    /// can filter by origin.
    Mod(&'static str),
}

impl DamageSource {
    #[inline]
    pub(crate) fn is_attack(self) -> bool {
        matches!(self, Self::PlayerAttack(_) | Self::MobAttack(_))
    }
}

/// An engine action a mod HostCall queued from inside a guest dispatch, where
/// the event bus is already borrowed and cannot be re-entered. `Game` drains
/// these at defined per-tick points (after every systems batch and before each
/// post-event drain) and routes each through the same funnel the engine's own
/// code uses, so registered pre handlers (i-frames, hurt tuning) still apply.
#[derive(Clone, Debug)]
pub(crate) enum ModAction {
    /// `Game::damage_player(amount, DamageSource::Mod(mod_id))`.
    DamagePlayer { amount: i32, mod_id: &'static str },
    /// Damage equal to the player's health at drain time, same funnel.
    KillPlayer { mod_id: &'static str },
    /// The mob-damage pipeline (`mob_damage_pre` → `Mobs::damage_mob` → death loot).
    /// `index` is storage order at drain time. Mod damage is not an attack, so
    /// it does not receive default knockback.
    DamageMob {
        index: usize,
        amount: f32,
        mod_id: &'static str,
        origin: Option<Vec3>,
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

/// Which container GUI a session opened/closed on (mirrors the game's container
/// targets, including the inventory's own 2×2 crafting grid).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    Inventory,
    CraftingTable,
    Furnace,
    Chest,
    FurnitureWorkbench,
    /// A mod-defined GUI session (Phase 5); the ABI mirror carries the kind's
    /// registered key string.
    Mod(crate::gui::GuiKind),
}

/// Observational events, queued at their site and drained FIFO at the post-queue
/// drain points (each tick-stage boundary) within the same tick.
#[derive(Copy, Clone, Debug)]
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
        kind: Mob,
        pos: Vec3,
    },
    MobSpawned {
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
    ContainerOpened {
        kind: ContainerKind,
        pos: Option<IVec3>,
    },
    ContainerClosed {
        kind: ContainerKind,
        pos: Option<IVec3>,
    },
    SectionGenerated {
        pos: SectionPos,
    },
    SectionLoaded {
        pos: SectionPos,
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
}

impl PostEventKind {
    pub(crate) const COUNT: usize = 11;
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
        }
    }
}
