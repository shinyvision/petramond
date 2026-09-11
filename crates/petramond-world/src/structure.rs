//! Compiled, stateful templates independent of generation and live-world policy.

use std::collections::BTreeMap;

use crate::block::{Block, ShapeState};
use crate::mathh::IVec3;
use crate::world::placement::authored::Turn;

mod compile;
mod load;
mod placement;
mod schema;

pub use load::{by_key, validate_catalog};
pub use placement::Placement;

#[cfg(test)]
mod tests;

/// Inclusive cell bounds, relative to a template's pivot or in world space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub min: IVec3,
    pub max: IVec3,
}

impl Bounds {
    pub fn contains(self, pos: IVec3) -> bool {
        pos.cmpge(self.min).all() && pos.cmple(self.max).all()
    }

    pub fn intersects(self, other: Self) -> bool {
        self.min.cmple(other.max).all() && self.max.cmpge(other.min).all()
    }
}

/// A materialized cell. Data uses the same namespaced, byte-valued surface as
/// runtime cell data, so templates do not define a separate persistence format.
#[derive(Clone)]
pub struct Cell {
    pub pos: IVec3,
    pub block: Block,
    pub state: ShapeState,
    pub data: BTreeMap<String, Vec<u8>>,
}

/// A named attachment point, facing out of its piece. Matching kinds can be
/// aligned without either template knowing the other's identity.
#[derive(Clone, Debug)]
pub struct Connector {
    pub name: String,
    pub kind: String,
    pub pos: IVec3,
    pub facing: crate::facing::Facing,
}

struct Variant {
    bounds: Bounds,
    cells: Vec<Cell>,
    connectors: Vec<Connector>,
}

/// All four rotations of a validated asset; expensive work happens at load.
pub struct Template {
    variants: [Variant; 4],
    requirements: Vec<mod_api::StructureRequirementData>,
}

impl Template {
    /// Parse and compile with an injected resolver, useful to editors and
    /// validators as well as the registry. No world state is read.
    pub fn parse(json: &str, resolve: impl Fn(&str) -> Option<Block>) -> Result<Self, String> {
        let raw: schema::Template = serde_json::from_str(json).map_err(|e| e.to_string())?;
        compile::compile(raw, resolve)
    }

    pub fn bounds(&self, turn: Turn) -> Bounds {
        self.variants[turn.index()].bounds
    }

    pub fn connectors(&self, turn: Turn) -> &[Connector] {
        &self.variants[turn.index()].connectors
    }

    /// Bounded metadata for mod candidate planning, without sending cell data.
    pub fn info(&self) -> mod_api::StructureInfoData {
        mod_api::StructureInfoData {
            requirements: self.requirements.clone(),
            bounds: Turn::ALL.map(|turn| {
                let bounds = self.bounds(turn);
                (bounds.min.to_array(), bounds.max.to_array())
            }),
            connectors: self
                .connectors(Turn::default())
                .iter()
                .map(|c| mod_api::StructureConnectorData {
                    name: c.name.clone(),
                    kind: c.kind.clone(),
                    pos: c.pos.to_array(),
                    normal: c.facing.dir().to_array(),
                })
                .collect(),
        }
    }

    /// Position the authored pivot at `origin`. Reject overflow before any
    /// consumer can apply part of an invalid placement.
    pub fn place(&self, origin: IVec3, turn: Turn) -> Result<Placement<'_>, String> {
        Placement::new(self, origin, turn)
    }
}
