use serde::{Deserialize, Serialize};

use super::{EventKind, EventPayload};
use crate::ids::{BlockId, ItemId, MobId};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRegion {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl CellRegion {
    pub fn contains(&self, cell: [i32; 3]) -> bool {
        (0..3).all(|i| self.min[i] <= cell[i] && cell[i] <= self.max[i])
    }

    pub fn overlaps(&self, min: [i32; 3], max: [i32; 3]) -> bool {
        (0..3).all(|i| self.min[i] <= max[i] && min[i] <= self.max[i])
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct EventFilter {
    pub blocks: Vec<BlockId>,
    pub items: Vec<ItemId>,
    pub mobs: Vec<MobId>,
    pub keys: Vec<String>,
    pub region: Option<CellRegion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterLane {
    Blocks,
    Items,
    Mobs,
    Keys,
    Region,
}

impl EventKind {
    pub fn carries(self, lane: FilterLane) -> bool {
        use EventKind as K;
        use FilterLane as L;
        match lane {
            L::Blocks => matches!(
                self,
                K::BlockPlacePre | K::BlockBreakPre | K::BlockPlaced | K::BlockBroken
            ),
            L::Items => matches!(
                self,
                K::ItemUsePre | K::ItemUsed | K::ItemPickedUp | K::ItemObtained | K::ProjectileHit
            ),
            L::Mobs => matches!(
                self,
                K::MobDamagePre
                    | K::MobDied
                    | K::MobSpawned
                    | K::MobTagAdded
                    | K::MobTagRemoved
                    | K::MobDamaged
            ),
            L::Keys => matches!(
                self,
                K::ModEvent
                    | K::MobTagAdded
                    | K::MobTagRemoved
                    | K::SchematicChosen
                    | K::SchematicPositioned
                    | K::ClientEvent
            ),
            L::Region => matches!(
                self,
                K::BlockPlacePre
                    | K::BlockBreakPre
                    | K::BlockPlaced
                    | K::BlockBroken
                    | K::InteractAttempt
                    | K::Interacted
                    | K::UseUnclaimed
                    | K::AttackAttempt
                    | K::ItemUsePre
                    | K::MobDied
                    | K::MobSpawned
                    | K::ItemPickedUp
                    | K::ProjectileHit
                    | K::ActorActed
                    | K::SchematicPositioned
                    | K::CellsEditPre
            ),
        }
    }

    pub fn echoes_payload(self) -> bool {
        matches!(
            self,
            EventKind::BlockBreakPre
                | EventKind::ProjectileHit
                | EventKind::MobDamagePre
                | EventKind::PlayerDamagePre
        )
    }
}

#[derive(Default)]
struct Facts<'a> {
    block: Option<BlockId>,
    item: Option<ItemId>,
    mob: Option<MobId>,
    key: Option<&'a str>,
    cell: Option<[i32; 3]>,
    cells: Option<([i32; 3], [i32; 3])>,
}

fn cell_of(pos: [f64; 3]) -> [i32; 3] {
    pos.map(|c| c.floor() as i32)
}

impl EventPayload {
    fn facts(&self) -> Facts<'_> {
        use EventPayload as P;
        let none = Facts::default();
        match self {
            P::BlockPlacePre { pos, block, .. }
            | P::BlockBreakPre { pos, block, .. }
            | P::BlockPlaced { pos, block }
            | P::BlockBroken { pos, block, .. } => Facts {
                block: Some(*block),
                cell: Some(*pos),
                ..none
            },
            P::InteractAttempt { block, .. }
            | P::Interacted { block, .. }
            | P::UseUnclaimed { block, .. }
            | P::AttackAttempt { block, .. } => Facts {
                cell: *block,
                ..none
            },
            P::ItemUsePre { item, target } => Facts {
                item: Some(*item),
                cell: *target,
                ..none
            },
            P::ItemUsed { item, .. } | P::ItemObtained { item, .. } => Facts {
                item: Some(*item),
                ..none
            },
            P::ItemPickedUp { item, pos, .. } => Facts {
                item: Some(*item),
                cell: Some(cell_of(*pos)),
                ..none
            },
            P::ProjectileHit { item, pos, .. } => Facts {
                item: Some(*item),
                cell: Some(cell_of(*pos)),
                ..none
            },
            P::MobDamagePre { kind, .. } | P::MobDamaged { kind, .. } => Facts {
                mob: Some(*kind),
                ..none
            },
            P::MobDied { kind, pos, .. } | P::MobSpawned { kind, pos, .. } => Facts {
                mob: Some(*kind),
                cell: Some(cell_of(*pos)),
                ..none
            },
            P::MobTagAdded { kind, key, .. } | P::MobTagRemoved { kind, key, .. } => Facts {
                mob: Some(*kind),
                key: Some(key),
                ..none
            },
            P::ModEvent { key, .. }
            | P::SchematicChosen { tag: key, .. }
            | P::ClientEvent { key, .. } => Facts {
                key: Some(key),
                ..none
            },
            P::SchematicPositioned { tag, origin, .. } => Facts {
                key: Some(tag),
                cell: Some(*origin),
                ..none
            },
            P::ActorActed { pos, .. } => Facts {
                cell: Some(*pos),
                ..none
            },
            P::CellsEditPre { min, max, .. } => Facts {
                cells: Some((*min, *max)),
                ..none
            },
            P::PlayerDamagePre { .. }
            | P::PlayerDamaged { .. }
            | P::PlayerDied
            | P::ContainerOpened { .. }
            | P::ContainerClosed { .. }
            | P::SectionGenerated { .. }
            | P::SectionLoaded { .. }
            | P::PlayerDismounted { .. } => none,
        }
    }
}

impl EventFilter {
    pub fn keys<S: Into<String>>(prefixes: impl IntoIterator<Item = S>) -> Self {
        Self {
            keys: prefixes.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn blocks(blocks: impl IntoIterator<Item = BlockId>) -> Self {
        Self {
            blocks: blocks.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn items(items: impl IntoIterator<Item = ItemId>) -> Self {
        Self {
            items: items.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn mobs(mobs: impl IntoIterator<Item = MobId>) -> Self {
        Self {
            mobs: mobs.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
            && self.items.is_empty()
            && self.mobs.is_empty()
            && self.keys.is_empty()
            && self.region.is_none()
    }

    fn lanes(&self) -> impl Iterator<Item = FilterLane> + '_ {
        [
            (!self.blocks.is_empty()).then_some(FilterLane::Blocks),
            (!self.items.is_empty()).then_some(FilterLane::Items),
            (!self.mobs.is_empty()).then_some(FilterLane::Mobs),
            (!self.keys.is_empty()).then_some(FilterLane::Keys),
            self.region.map(|_| FilterLane::Region),
        ]
        .into_iter()
        .flatten()
    }

    pub fn check(&self, kind: EventKind) -> Result<(), String> {
        if let Some(lane) = self.lanes().find(|&lane| !kind.carries(lane)) {
            return Err(format!(
                "{kind:?} events carry no {lane:?} fact to filter on"
            ));
        }
        if let Some(r) = self.region {
            if (0..3).any(|i| r.min[i] > r.max[i]) {
                return Err(format!("event filter region is inverted: {r:?}"));
            }
        }
        Ok(())
    }

    pub fn matches(&self, payload: &EventPayload) -> bool {
        if self.is_empty() {
            return true;
        }
        let facts = payload.facts();
        fn lane<T: PartialEq>(wanted: &[T], fact: Option<T>) -> bool {
            wanted.is_empty() || fact.is_some_and(|f| wanted.contains(&f))
        }
        lane(&self.blocks, facts.block)
            && lane(&self.items, facts.item)
            && lane(&self.mobs, facts.mob)
            && (self.keys.is_empty()
                || facts
                    .key
                    .is_some_and(|key| self.keys.iter().any(|p| key.starts_with(p.as_str()))))
            && self.region.is_none_or(|r| match (facts.cell, facts.cells) {
                (Some(cell), _) => r.contains(cell),
                (None, Some((min, max))) => r.overlaps(min, max),
                (None, None) => false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::EntityRef;
    use crate::ids::PlayerId;

    fn placed(block: u16, pos: [i32; 3]) -> EventPayload {
        EventPayload::BlockPlaced {
            pos,
            block: BlockId(block),
        }
    }

    fn mod_event(key: &str) -> EventPayload {
        EventPayload::ModEvent {
            key: key.into(),
            data: Vec::new(),
        }
    }

    #[test]
    fn an_empty_filter_admits_everything() {
        let f = EventFilter::default();
        assert!(f.is_empty());
        assert!(f.matches(&placed(3, [0, 0, 0])));
        assert!(f.matches(&EventPayload::PlayerDied));
        for kind in [
            EventKind::PlayerDied,
            EventKind::ModEvent,
            EventKind::BlockPlaced,
        ] {
            assert_eq!(f.check(kind), Ok(()));
        }
    }

    #[test]
    fn id_lanes_admit_only_listed_ids() {
        let f = EventFilter::blocks([BlockId(3), BlockId(9)]);
        assert!(f.matches(&placed(3, [0, 0, 0])));
        assert!(f.matches(&placed(9, [0, 0, 0])));
        assert!(!f.matches(&placed(4, [0, 0, 0])));
        let items = EventFilter::items([ItemId(7)]);
        let used = |item| EventPayload::ItemUsePre {
            item: ItemId(item),
            target: None,
        };
        assert!(items.matches(&used(7)));
        assert!(!items.matches(&used(8)));
    }

    #[test]
    fn key_lane_matches_prefixes() {
        let f = EventFilter::keys(["weather:"]);
        assert!(f.matches(&mod_event("weather:field")));
        assert!(!f.matches(&mod_event("farming:harvest")));
        let exact = EventFilter::keys(["farming:harvest"]);
        assert!(exact.matches(&mod_event("farming:harvest")));
        assert!(!exact.matches(&mod_event("farming:till")));
    }

    #[test]
    fn region_lane_needs_a_cell_inside_the_box() {
        let f = EventFilter {
            region: Some(CellRegion {
                min: [0, 0, 0],
                max: [15, 15, 15],
            }),
            ..EventFilter::default()
        };
        assert!(f.matches(&placed(1, [15, 0, 3])));
        assert!(!f.matches(&placed(1, [16, 0, 3])));
        let click = |block| EventPayload::InteractAttempt {
            block,
            face: None,
            mob: None,
            player: PlayerId(1),
        };
        assert!(f.matches(&click(Some([1, 1, 1]))));
        assert!(!f.matches(&click(None)), "a click at a mob carries no cell");
        let edit = |min, max| EventPayload::CellsEditPre {
            min,
            max,
            cells: 1,
            actor: EntityRef::Player(PlayerId(1)),
        };
        assert!(f.matches(&edit([10, 10, 10], [30, 30, 30])));
        assert!(!f.matches(&edit([20, 0, 0], [30, 5, 5])));
    }

    #[test]
    fn lanes_combine_as_and() {
        let f = EventFilter {
            blocks: vec![BlockId(3)],
            region: Some(CellRegion {
                min: [0, 0, 0],
                max: [1, 1, 1],
            }),
            ..EventFilter::default()
        };
        assert!(f.matches(&placed(3, [1, 1, 1])));
        assert!(!f.matches(&placed(3, [2, 1, 1])));
        assert!(!f.matches(&placed(4, [1, 1, 1])));
    }

    #[test]
    fn check_refuses_lanes_the_kind_never_carries() {
        let blocks = EventFilter::blocks([BlockId(1)]);
        assert_eq!(blocks.check(EventKind::BlockPlaced), Ok(()));
        assert!(blocks.check(EventKind::ModEvent).is_err());
        assert!(EventFilter::keys(["a:"])
            .check(EventKind::BlockBroken)
            .is_err());
        let inverted = EventFilter {
            region: Some(CellRegion {
                min: [1, 0, 0],
                max: [0, 0, 0],
            }),
            ..EventFilter::default()
        };
        assert!(inverted.check(EventKind::BlockPlaced).is_err());
    }

    #[test]
    fn every_carried_lane_is_offered_by_its_payloads() {
        let hit = EventPayload::ProjectileHit {
            entity: 1,
            item: ItemId(5),
            target: crate::ProjectileTarget::Mob(2),
            pos: [0.5, 0.5, 0.5],
            vel: [0.0; 3],
            fate: crate::ProjectileFate::Drop,
        };
        assert!(EventKind::ProjectileHit.carries(FilterLane::Items));
        assert!(EventFilter::items([ItemId(5)]).matches(&hit));
        assert!(!EventFilter::items([ItemId(6)]).matches(&hit));
    }

    #[test]
    fn only_mutable_kinds_echo_their_payload() {
        assert!(EventKind::MobDamagePre.echoes_payload());
        assert!(EventKind::BlockBreakPre.echoes_payload());
        assert!(!EventKind::BlockPlaced.echoes_payload());
        assert!(!EventKind::InteractAttempt.echoes_payload());
        assert!(!EventKind::ModEvent.echoes_payload());
    }
}
