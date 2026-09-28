use std::collections::{BTreeMap, BTreeSet};

use crate::block::{Block, CellCodec, ShapeState};
use crate::block_state::{EntityFront, LogAxis};
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::{CellWrite, PlacementPlan};

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

/// One material's writes relative to its anchor at `turn`, laid out by the block's own shape
/// family. Templates and generation output both expand materials through this, so an authored
/// stair, door or bed lands the same way whichever of them wrote it.
pub fn layout(
    block: Block,
    properties: &BTreeMap<String, String>,
    turn: Turn,
) -> Result<Vec<CellWrite>, String> {
    let mut inputs = Inputs::new(IVec3::ZERO, turn, properties);
    let plan = block
        .shape_kind_def()
        .placement
        .authored_plan(block, &mut inputs)?;
    inputs.finish()?;
    Ok(plan.writes)
}

#[derive(Clone)]
pub struct Cell {
    pub pos: IVec3,
    pub block: Block,
    pub state: ShapeState,
    pub data: BTreeMap<String, Vec<u8>>,
}

/// Anchored layouts expanded into cells, later placements replacing earlier ones cell by cell.
#[derive(Default)]
pub struct Expansion {
    cells: Vec<Cell>,
    /// The placement each cell came from.
    owners: Vec<u32>,
    owner_sizes: Vec<u32>,
}

impl Expansion {
    /// Room for `writes` cell writes before it grows.
    pub fn with_capacity(writes: usize) -> Expansion {
        Expansion {
            cells: Vec::with_capacity(writes),
            owners: Vec::with_capacity(writes),
            owner_sizes: Vec::with_capacity(writes),
        }
    }

    pub fn place(&mut self, anchor: IVec3, layout: &[CellWrite]) {
        let owner = self.owner_sizes.len() as u32;
        self.owner_sizes.push(layout.len() as u32);
        for write in layout {
            self.cells.push(Cell {
                pos: anchor + write.cell,
                block: write.block,
                state: write.state,
                data: BTreeMap::new(),
            });
            self.owners.push(owner);
        }
    }

    pub fn writes(&self) -> usize {
        self.cells.len()
    }

    /// One cell per position, the last placement's. Fails when an overlay left only PART of a
    /// multi-cell object standing: half a door is never a valid world state.
    pub fn finish(self) -> Result<Expanded, String> {
        if !self.repeats_a_position() {
            return Ok(Expanded {
                cells: self.cells,
                index: None,
                lookups: 0,
            });
        }
        let mut last: rustc_hash::FxHashMap<Pos, u32> =
            rustc_hash::FxHashMap::with_capacity_and_hasher(self.cells.len(), Default::default());
        let mut replaced = vec![false; self.cells.len()];
        for (i, cell) in self.cells.iter().enumerate() {
            if let Some(earlier) = last.insert(Pos(cell.pos.to_array()), i as u32) {
                replaced[earlier as usize] = true;
            }
        }
        let mut surviving = vec![0u32; self.owner_sizes.len()];
        let mut cells: Vec<Cell> = Vec::with_capacity(last.len());
        for ((cell, owner), replaced) in self.cells.into_iter().zip(self.owners).zip(replaced) {
            if !replaced {
                surviving[owner as usize] += 1;
                cells.push(cell);
            }
        }
        if surviving
            .iter()
            .zip(&self.owner_sizes)
            .any(|(&left, &total)| left != 0 && left != total)
        {
            return Err("an overlay cuts through a multi-cell object".into());
        }
        Ok(Expanded {
            cells,
            index: None,
            lookups: 0,
        })
    }

    /// Whether two writes land on the same cell. Checked against a bitset over the writes'
    /// bounding box, so the common output (every cell written once) needs no hashing at all.
    fn repeats_a_position(&self) -> bool {
        const DENSE_MAX: i64 = 1 << 18;
        let Some(first) = self.cells.first() else {
            return false;
        };
        let (mut lo, mut hi) = (first.pos.to_array(), first.pos.to_array());
        for w in &self.cells {
            let pos = w.pos.to_array();
            for a in 0..3 {
                lo[a] = lo[a].min(pos[a]);
                hi[a] = hi[a].max(pos[a]);
            }
        }
        let side = |a: usize| i64::from(hi[a]) - i64::from(lo[a]) + 1;
        let volume = side(0) * side(1) * side(2);
        if volume > DENSE_MAX {
            return true;
        }
        let (sx, sy) = (side(0), side(1));
        let mut seen = vec![0u64; (volume as usize).div_ceil(64)];
        for w in &self.cells {
            let pos = w.pos.to_array();
            let d = |a: usize| i64::from(pos[a]) - i64::from(lo[a]);
            let i = ((d(2) * sy + d(1)) * sx + d(0)) as usize;
            let bit = 1u64 << (i % 64);
            if seen[i / 64] & bit != 0 {
                return true;
            }
            seen[i / 64] |= bit;
        }
        false
    }
}

/// A position hashed as one word.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Pos([i32; 3]);

impl std::hash::Hash for Pos {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let [x, y, z] = self.0.map(|v| u64::from(v as u32));
        state.write_u64(x ^ z.rotate_left(32) ^ y.rotate_left(16));
    }
}

/// Expanded cells, one per position, in placement order.
pub struct Expanded {
    cells: Vec<Cell>,
    index: Option<rustc_hash::FxHashMap<Pos, u32>>,
    lookups: u32,
}

impl Expanded {
    /// The cell at `pos`. The first few lookups scan (an output attaches data to a handful of
    /// cells); later ones build an index.
    pub fn get_mut(&mut self, pos: [i32; 3]) -> Option<&mut Cell> {
        const SCANS: u32 = 8;
        if self.index.is_none() && self.lookups < SCANS {
            self.lookups += 1;
            let pos = IVec3::from(pos);
            return self.cells.iter_mut().rev().find(|c| c.pos == pos);
        }
        let cells = &self.cells;
        let index = self.index.get_or_insert_with(|| {
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| (Pos(c.pos.to_array()), i as u32))
                .collect()
        });
        let i = *index.get(&Pos(pos))?;
        self.cells.get_mut(i as usize)
    }

    pub fn into_cells(self) -> Vec<Cell> {
        self.cells
    }
}
