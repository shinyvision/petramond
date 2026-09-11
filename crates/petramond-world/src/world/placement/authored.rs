//! Semantic state authoring; families retain ownership of their cell codecs.

use std::collections::{BTreeMap, BTreeSet};

use crate::block::{Block, CellCodec, ShapeState};
use crate::block_state::{EntityFront, LogAxis};
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::PlacementPlan;

/// A clockwise horizontal rotation, viewed from above. Coordinates rotate
/// around a cell centre, so negative positions and object anchors stay exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn(u8);

impl Turn {
    pub const ALL: [Self; 4] = [Self(0), Self(1), Self(2), Self(3)];

    pub fn new(quarters: u8) -> Result<Self, String> {
        if quarters < 4 {
            Ok(Self(quarters))
        } else {
            Err("rotation must be 0..=3 quarter turns".into())
        }
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn apply(self, p: IVec3) -> IVec3 {
        match self.0 {
            0 => p,
            1 => IVec3::new(-p.z, p.y, p.x),
            2 => IVec3::new(-p.x, p.y, -p.z),
            _ => IVec3::new(p.z, p.y, -p.x),
        }
    }

    pub fn facing(self, facing: Facing) -> Facing {
        Facing::from_horizontal_normal(self.apply(facing.dir())).unwrap()
    }
}

/// A family consumes only properties it understands. Unconsumed properties
/// fail compilation, preventing misspellings from silently changing geometry.
pub struct Inputs<'a> {
    pub anchor: IVec3,
    pub turn: Turn,
    properties: &'a BTreeMap<String, String>,
    consumed: BTreeSet<&'a str>,
}

impl<'a> Inputs<'a> {
    pub fn new(anchor: IVec3, turn: Turn, properties: &'a BTreeMap<String, String>) -> Self {
        Self {
            anchor,
            turn,
            properties,
            consumed: BTreeSet::new(),
        }
    }

    pub fn property<'b>(&mut self, key: &str, default: &'b str) -> &'b str
    where
        'a: 'b,
    {
        match self.properties.get_key_value(key) {
            Some((key, value)) => {
                self.consumed.insert(key.as_str());
                value
            }
            None => default,
        }
    }

    pub fn facing(&mut self) -> Result<Facing, String> {
        let facing = match self.property("facing", "north") {
            "north" => Facing::North,
            "east" => Facing::East,
            "south" => Facing::South,
            "west" => Facing::West,
            value => return Err(format!("unknown facing '{value}'")),
        };
        Ok(self.turn.facing(facing))
    }

    pub fn finish(self) -> Result<(), String> {
        if let Some(key) = self
            .properties
            .keys()
            .find(|key| !self.consumed.contains(key.as_str()))
        {
            Err(format!("unsupported state property '{key}'"))
        } else {
            Ok(())
        }
    }

    pub fn general(&mut self, block: Block) -> Result<PlacementPlan, String> {
        let state = if block.is_log() {
            let axis = match self.property("axis", "y") {
                "x" => {
                    if self.turn.index().is_multiple_of(2) {
                        LogAxis::X
                    } else {
                        LogAxis::Z
                    }
                }
                "y" => LogAxis::Y,
                "z" => {
                    if self.turn.index().is_multiple_of(2) {
                        LogAxis::Z
                    } else {
                        LogAxis::X
                    }
                }
                value => return Err(format!("unknown log axis '{value}'")),
            };
            axis.to_cell()
        } else if block.directional_view() {
            EntityFront(self.facing()?).to_cell()
        } else {
            ShapeState::NONE
        };
        Ok(PlacementPlan::single(self.anchor, block, state))
    }
}
