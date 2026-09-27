use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::player::PlayerId;
use petramond_math::math::IVec3;

use super::{
    ActionOutcome, BlockDelta, BlockDrawDelta, CellKvDelta, ItemLane, MenuSyncMsg, MobLane,
    PlayerActionKind, PlayerLane, SelfEvents, SelfState, SleepTally, WorldEventMsg,
};

pub trait TickPart: Sized {
    fn into_section(self) -> TickSection;
    fn of(section: &TickSection) -> Option<&Self>;
    fn of_mut(section: &mut TickSection) -> Option<&mut Self>;
}

macro_rules! tick_sections {
    ($($(#[$doc:meta])* $variant:ident($ty:ty) => $get:ident;)*) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[allow(clippy::large_enum_variant)]
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
    Creative(Vec<crate::schematic::CreativeReply>) => creative;
    Schematics(Vec<crate::schematic::share::SchematicNotice>) => schematics;
    BlockDeltas(Vec<BlockDelta>) => block_deltas;
    BlockDraws(Vec<BlockDrawDelta>) => block_draws;
    CellKvDeltas(Vec<CellKvDelta>) => cell_kv_deltas;
    Mobs(MobLane) => mobs;
    Items(ItemLane) => items;
    Players(PlayerLane) => players;
    PlayerActions(Arc<[(PlayerId, PlayerActionKind)]>) => player_actions;
    SleepTally(SleepTally) => sleep_tally;
    ActionOutcomes(Vec<ActionOutcome>) => action_outcomes;
    SelfState(SelfState) => self_state;
    MenuSync(MenuSyncMsg) => menu_sync;
    Env(Vec<(String, [f32; 4])>) => env;
    OpenChests(Vec<IVec3>) => open_chests;
    Events(Vec<WorldEventMsg>) => events;
    SelfEvents(SelfEvents) => self_events;
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TickUpdate {
    pub tick: u64,
    pub clock: u64,
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

    pub fn push<T: TickPart>(&mut self, part: T) {
        self.sections.push(part.into_section());
    }

    pub fn push_list<T>(&mut self, list: Vec<T>)
    where
        Vec<T>: TickPart,
    {
        if !list.is_empty() {
            self.push(list);
        }
    }

    pub fn with<T: TickPart>(mut self, part: T) -> Self {
        self.push(part);
        self
    }

    pub fn part<T: TickPart>(&self) -> Option<&T> {
        self.sections.iter().find_map(T::of)
    }

    pub fn part_mut<T: TickPart>(&mut self) -> Option<&mut T> {
        self.sections.iter_mut().find_map(T::of_mut)
    }
}
