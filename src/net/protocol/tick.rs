//! One tick window's replication batch as a list of typed SECTIONS.
//!
//! Each replicated feature owns one [`TickSection`] variant, declared once in
//! the `tick_sections!` list below: the variant, its payload type and its
//! accessor. The server pushes the sections it has something to say in (an
//! empty lane, an unchanged environment, a quiet event queue cost nothing on
//! the wire); the id remap and the client's apply are exhaustive matches over
//! the variants, so a new feature is one line here plus the arms the compiler
//! then asks for — never a field threaded through a monolith.
//!
//! Sections apply in LIST order, and the server emits them in the canonical
//! order of the list below: world deltas before the rows and events that may
//! reference their cells, a block write before the draw set and KV written
//! after it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::player::PlayerId;
use petramond_math::math::IVec3;

use super::{
    ActionOutcome, BlockDelta, BlockDrawDelta, CellKvDelta, ItemLane, MenuSyncMsg, MobLane,
    PlayerActionKind, PlayerLane, SelfEvents, SelfState, SleepTally, WorldEventMsg,
};

/// A payload type that is exactly one [`TickSection`] variant.
pub trait TickPart: Sized {
    fn into_section(self) -> TickSection;
    fn of(section: &TickSection) -> Option<&Self>;
    fn of_mut(section: &mut TickSection) -> Option<&mut Self>;
}

macro_rules! tick_sections {
    ($($(#[$doc:meta])* $variant:ident($ty:ty) => $get:ident;)*) => {
        /// One feature's part of a [`TickUpdate`].
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        pub enum TickSection {
            $($(#[$doc])* $variant($ty),)*
        }

        $(impl TickPart for $ty {
            fn into_section(self) -> TickSection {
                TickSection::$variant(self)
            }
            fn of(section: &TickSection) -> Option<&Self> {
                match section {
                    TickSection::$variant(v) => Some(v),
                    _ => None,
                }
            }
            fn of_mut(section: &mut TickSection) -> Option<&mut Self> {
                match section {
                    TickSection::$variant(v) => Some(v),
                    _ => None,
                }
            }
        })*

        impl TickUpdate {
            $(
                #[doc = concat!("The [`TickSection::", stringify!($variant), "`] section, if the batch carries one.")]
                pub fn $get(&self) -> Option<&$ty> {
                    self.part::<$ty>()
                }
            )*
        }
    };
}

tick_sections! {
    /// Schematic/creative replies for the recipient: applied first, they are
    /// read models the rest of the batch never depends on.
    Creative(Vec<crate::schematic::CreativeReply>) => creative;
    /// Schematic choices, positionings, archive streams and ghosts for this
    /// recipient.
    Schematics(Vec<crate::schematic::share::SchematicNotice>) => schematics;
    /// This window's coalesced cell changes in sections the recipient holds.
    BlockDeltas(Vec<BlockDelta>) => block_deltas;
    /// This window's changed mod draw sets (after the block writes: a block
    /// write drops the cell's set on both sides).
    BlockDraws(Vec<BlockDrawDelta>) => block_draws;
    /// This window's per-cell mod KV changes (after the block writes, which
    /// wipe a cell's KV on both sides).
    CellKvDeltas(Vec<CellKvDelta>) => cell_kv_deltas;
    /// The mobs in the recipient's interest: spawns, despawns and updates
    /// against what it already tracks (see [`super::EntityLane`]). The entity
    /// lanes select rows out of tables the server builds once per window, so
    /// an entity tracked by many recipients costs one row plus a refcount
    /// bump per batch in process; on the wire each connection encodes only
    /// its own selection.
    Mobs(MobLane) => mobs;
    /// The dropped items in the recipient's interest.
    Items(ItemLane) => items;
    /// The players in the recipient's interest — always including the
    /// recipient itself (the client reads its own mount from that row).
    Players(PlayerLane) => players;
    /// This window's one-shot animation events of the players in the
    /// recipient's interest, in emission order.
    PlayerActions(Arc<[(PlayerId, PlayerActionKind)]>) => player_actions;
    /// How many connected players are asleep, out of how many — every
    /// session, tracked or not. Rides every batch.
    SleepTally(SleepTally) => sleep_tally;
    /// Answers to this recipient's `ClientRequestId`s, in emission order —
    /// ahead of the authoritative state they answer for.
    ActionOutcomes(Vec<ActionOutcome>) => action_outcomes;
    /// The recipient's own player state. Rides every batch.
    SelfState(SelfState) => self_state;
    /// The recipient's menu-session view, only when it changed.
    MenuSync(MenuSyncMsg) => menu_sync;
    /// The server `WorldEnvironment`'s named shader params that changed
    /// since the last batch (absent = nothing changed). Names are strings —
    /// engine `petramond:*` keys and mod namespaces; the client writes them
    /// into its REPLICA world's environment, which the renderer reads.
    Env(Vec<(String, [f32; 4])>) => env;
    /// Every chest with at least one open screen (any player) — the lid
    /// animation's full state. ABSENT means no chest is open.
    OpenChests(Vec<IVec3>) => open_chests;
    /// This window's world-anchored events the recipient can perceive, in
    /// emission order.
    Events(Vec<WorldEventMsg>) => events;
    /// The recipient's own per-tick one-shots.
    SelfEvents(SelfEvents) => self_events;
}

/// One executed server tick's replication to one client. At most one per pump
/// (states as of the latest executed tick); the client applies it atomically
/// and interpolates between consecutive updates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TickUpdate {
    pub tick: u64,
    pub clock: u64,
    /// The features this batch carries, in apply order (see the module doc).
    pub sections: Vec<TickSection>,
}

impl TickUpdate {
    pub fn new(tick: u64, clock: u64) -> Self {
        TickUpdate {
            tick,
            clock,
            sections: Vec::new(),
        }
    }

    /// Append one section.
    pub fn push<T: TickPart>(&mut self, part: T) {
        self.sections.push(part.into_section());
    }

    /// Append a list section unless it is empty — an empty list says nothing.
    pub fn push_list<T>(&mut self, list: Vec<T>)
    where
        Vec<T>: TickPart,
    {
        if !list.is_empty() {
            self.push(list);
        }
    }

    /// Builder form of [`push`](Self::push).
    pub fn with<T: TickPart>(mut self, part: T) -> Self {
        self.push(part);
        self
    }

    /// The section of type `T`, if the batch carries one.
    pub fn part<T: TickPart>(&self) -> Option<&T> {
        self.sections.iter().find_map(T::of)
    }

    /// The section of type `T`, mutably.
    pub fn part_mut<T: TickPart>(&mut self) -> Option<&mut T> {
        self.sections.iter_mut().find_map(T::of_mut)
    }
}
