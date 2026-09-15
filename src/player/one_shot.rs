//! The engine's own one-shot gesture vocabulary — a swing, a break, a
//! place, an interact, a throw, per hand — resolved ONCE per registered rig
//! into that rig's graph event ids (`main.swing`, `off.place`, …). Every
//! layer above fires them through the same `(rig, event)` lane a mod's
//! `FirePlayerAnimatorEvent` rides: the server replicates them to observers,
//! the client predicts them onto its own rigs, the render drivers only ever
//! see graph events. A rig whose graph declares no such name simply never
//! hears that gesture.

use std::sync::LazyLock;

use petramond_world::inventory::Hand;

use super::rigs::{self, RigId};

/// The hand gestures the engine itself fires as edges. Level-derived edges
/// (`mine`, `eat`, `equip`) are not here: the drivers compute those from
/// the hand's levels each frame.
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

    /// The graph event name under a hand's prefix.
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

/// Per rig, per hand, per gesture: the graph event id, or `None` where the
/// rig's graph declares no such name.
static TABLE: LazyLock<Vec<[[Option<u16>; 5]; 2]>> = LazyLock::new(|| {
    rigs::all()
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
        .collect()
});

/// The event id `kind` from `hand` fires on `rig`, if its graph has one.
pub fn resolve(rig: RigId, hand: Hand, kind: OneShot) -> Option<u16> {
    TABLE.get(rig.index())?[hand_slot(hand)][kind.slot()]
}

/// Every rig's event for `kind` from `hand` — the `(rig, event)` rows a
/// fired gesture becomes, in rig id order.
pub fn fired(hand: Hand, kind: OneShot) -> impl Iterator<Item = (RigId, u16)> {
    TABLE
        .iter()
        .enumerate()
        .filter_map(move |(i, per_hand)| {
            per_hand[hand_slot(hand)][kind.slot()].map(|event| (RigId(i as u16), event))
        })
}
