use petramond_world::inventory::Hand;

use super::rigs::{self, RigId};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum OneShot {
    Swing,
    Break,
    Place,
    Interact,
    Throw,
}

impl OneShot {
    pub const ALL: [Self; 5] = [
        Self::Swing,
        Self::Break,
        Self::Place,
        Self::Interact,
        Self::Throw,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Swing => "swing",
            Self::Break => "break",
            Self::Place => "place",
            Self::Interact => "interact",
            Self::Throw => "throw",
        }
    }

    fn slot(self) -> usize {
        self as usize
    }
}

fn hand_prefix(hand: Hand) -> &'static str {
    match hand {
        Hand::Main => "main",
        Hand::Off => "off",
    }
}

fn hand_slot(hand: Hand) -> usize {
    match hand {
        Hand::Main => 0,
        Hand::Off => 1,
    }
}

type EventTable = Vec<[[Option<u16>; 5]; 2]>;

static TABLE: petramond_world::content::Slot<EventTable> =
    petramond_world::content::Slot::new("rig gesture events", &[], resolve_events);

fn resolve_events(_: &petramond_world::content::ContentRegistry) -> Result<EventTable, String> {
    Ok(rigs::all()
        .iter()
        .map(|rig| {
            [Hand::Main, Hand::Off].map(|hand| {
                OneShot::ALL.map(|kind| {
                    let graph = rig.graph.as_ref()?;
                    let name = format!("{}.{}", hand_prefix(hand), kind.name());
                    u16::try_from(graph.event(&name)?.index()).ok()
                })
            })
        })
        .collect())
}

pub fn resolve(rig: RigId, hand: Hand, kind: OneShot) -> Option<u16> {
    TABLE.current().get(rig.index())?[hand_slot(hand)][kind.slot()]
}

pub fn fired(hand: Hand, kind: OneShot) -> impl Iterator<Item = (RigId, u16)> {
    TABLE
        .current()
        .iter()
        .enumerate()
        .filter_map(move |(i, per_hand)| {
            per_hand[hand_slot(hand)][kind.slot()].map(|event| (RigId(i as u16), event))
        })
}
