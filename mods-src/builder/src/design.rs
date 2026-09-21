//! A project's design compiled into construction units in build order.
//!
//! Compilation reads the world-held schematic one stored section per call
//! and asks the registry what each distinct record costs, so a huge design
//! compiles over many ticks instead of in one. A unit is a position and an
//! index into the design's records; everything a record asks of
//! construction is kept once per record.

use crate::fx::{HashMap, HashSet};

use mod_sdk::*;

pub use mod_sdk::paged;

/// What one distinct record asks of construction.
#[derive(Clone, Debug)]
pub enum Plan {
    /// The cell must end up empty.
    Air,
    /// A cell of an object another cell anchors; that unit builds it.
    Member,
    /// An object anchored at this record's cell.
    Build {
        /// Cells relative to the anchor.
        footprint: Vec<[i32; 3]>,
        /// Sorted after full blocks of its layer: panes, lanterns, doors and
        /// other shapes that usually hang on or lean against something.
        attachment: bool,
        /// Laid after everything around it: leaves live only near wood.
        late: bool,
        /// Breaks once what holds it goes, so it never hangs on scaffolding.
        fragile: bool,
        /// A single-cell panel the full height of its cell (a window pane):
        /// glazed once the rest stands, the opening a way through till then.
        glazing: bool,
    },
    Unsupported(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    pub pos: [i32; 3],
    pub record: u32,
}

pub struct Design {
    pub asset: SchematicId,
    pub origin: [i32; 3],
    pub turns: u8,
    pub title: String,
    sections: u32,
    next_section: u32,
    pub records: Vec<BlockRecord>,
    pub plans: Vec<Plan>,
    /// What building each record costs, whatever stands in its place now.
    costs: Vec<Vec<ItemStackData>>,
    pub units: Vec<Unit>,
    /// Every cell a unit governs, for keeping scaffolding out of the design.
    pub governed: HashSet<[i32; 3]>,
    /// The cells objects will fill (governed cells that must end up empty
    /// are left out): nowhere for the golem to stand.
    pub filled: HashSet<[i32; 3]>,
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub complete: bool,
    record_index: HashMap<BlockRecord, u32>,
    at: HashMap<[i32; 3], usize>,
    tagged: HashMap<&'static str, HashSet<BlockId>>,
    passages: HashSet<usize>,
    /// The columns the design's floor covers: the fullest of its lowest few
    /// layers. Inside the house or outside it, by column.
    floor: HashSet<[i32; 2]>,
}

/// Blocks laid last: they decay unless what they live beside already stands.
const LATE_TAG: &str = "leaves";
const FRAGILE_TAG: &str = "fragile";
/// The row a cell the design keeps empty is recorded as.
const EMPTY: &str = "petramond:air";

pub enum Progress {
    Compiling,
    Ready,
    Failed(String),
}

impl Design {
    /// Whether `cell` lies in the design's box or the ground the golem works
    /// it from around it.
    pub fn near(&self, cell: [i32; 3]) -> bool {
        const AROUND: i32 = 8;
        (0..3).all(|i| cell[i] >= self.min[i] - AROUND && cell[i] <= self.max[i] + AROUND)
    }

    pub fn new(asset: SchematicId, origin: [i32; 3], turns: u8) -> Self {
        Self {
            asset,
            origin,
            turns,
            title: String::new(),
            sections: 0,
            next_section: 0,
            records: Vec::new(),
            plans: Vec::new(),
            costs: Vec::new(),
            units: Vec::new(),
            governed: HashSet::default(),
            filled: HashSet::default(),
            min: origin,
            max: origin,
            complete: false,
            record_index: HashMap::default(),
            at: HashMap::default(),
            tagged: HashMap::default(),
            passages: HashSet::default(),
            floor: HashSet::default(),
        }
    }

    /// The rows of natural overgrowth (the leaves tag): what a builder trims
    /// away where it crowds or hides the work.
    pub fn overgrowth(&mut self) -> Vec<BlockId> {
        self.tagged
            .entry(LATE_TAG)
            .or_insert_with(|| blocks_by_tag(LATE_TAG).into_iter().collect())
            .iter()
            .copied()
            .collect()
    }

    fn tagged(&mut self, tag: &'static str, block: BlockId) -> bool {
        self.tagged
            .entry(tag)
            .or_insert_with(|| blocks_by_tag(tag).into_iter().collect())
            .contains(&block)
    }

    pub fn matches(&self, asset: SchematicId, origin: [i32; 3], turns: u8) -> bool {
        self.asset == asset && self.origin == origin && self.turns == turns
    }

    /// Stored sections compiled so far.
    pub fn compiled(&self) -> u32 {
        self.next_section
    }

    /// Compile up to `sections` more stored sections.
    pub fn compile(&mut self, sections: u32) -> Progress {
        if self.complete {
            return Progress::Ready;
        }
        if self.next_section == 0 && self.sections == 0 {
            match schematic_info(self.asset) {
                SchematicLookup::Missing => {
                    return Progress::Failed("The schematic is missing".into())
                }
                SchematicLookup::Failed { reason } => return Progress::Failed(reason),
                SchematicLookup::Loading => return Progress::Compiling,
                SchematicLookup::Ready(info) => {
                    self.title = info.title;
                    self.sections = info.sections;
                    let size = turned_size(info.size, self.turns);
                    self.max = std::array::from_fn(|i| self.origin[i] + size[i] - 1);
                }
            }
        }
        let end = (self.next_section + sections).min(self.sections);
        while self.next_section < end {
            let Some(cells) = schematic_cells(self.asset, self.next_section, self.turns) else {
                return Progress::Compiling;
            };
            self.add_section(cells);
            self.next_section += 1;
        }
        if self.next_section == self.sections {
            self.add_rooms();
            self.finish();
            return Progress::Ready;
        }
        Progress::Compiling
    }

    fn add_section(&mut self, section: SchematicCellsData) {
        let fresh: Vec<BlockRecord> = section
            .palette
            .iter()
            .filter(|r| !self.record_index.contains_key(*r))
            .cloned()
            .collect();
        let mut seen = HashSet::default();
        let fresh: Vec<BlockRecord> = fresh
            .into_iter()
            .filter(|r| seen.insert(r.clone()))
            .collect();
        if !fresh.is_empty() {
            let plans = paged(fresh.clone(), block_record_plans);
            let shapes = record_shapes(&fresh);
            for ((record, plan), (full, panel)) in fresh.into_iter().zip(plans).zip(shapes) {
                let index = self.records.len() as u32;
                let block = resolve_block(&record.block);
                let late = block.is_some_and(|b| self.tagged(LATE_TAG, b));
                let fragile = block.is_some_and(|b| self.tagged(FRAGILE_TAG, b));
                self.costs.push(match &plan {
                    RecordPlan::Unit { cost, .. } => cost.clone(),
                    _ => Vec::new(),
                });
                self.plans.push(match plan {
                    RecordPlan::Air => Plan::Air,
                    RecordPlan::Member { .. } => Plan::Member,
                    RecordPlan::Unit { footprint, .. } => Plan::Build {
                        glazing: panel && footprint.len() == 1,
                        footprint,
                        attachment: !full,
                        late,
                        fragile,
                    },
                    RecordPlan::Unsupported { reason } => Plan::Unsupported(reason),
                });
                self.record_index.insert(record.clone(), index);
                self.records.push(record);
            }
        }
        for (local, palette) in section.cells {
            let Some(record) = section
                .palette
                .get(palette as usize)
                .and_then(|r| self.record_index.get(r))
                .copied()
            else {
                continue;
            };
            let pos = std::array::from_fn(|i| self.origin[i] + local[i]);
            match &self.plans[record as usize] {
                Plan::Member => {}
                Plan::Build { footprint, .. } => {
                    for offset in footprint {
                        self.governed.insert(add(pos, *offset));
                        self.filled.insert(add(pos, *offset));
                    }
                    self.units.push(Unit { pos, record });
                }
                _ => {
                    self.governed.insert(pos);
                    self.units.push(Unit { pos, record });
                }
            }
        }
    }

    /// The cells a house keeps empty without recording them: a sparse design
    /// leaves its rooms out, and set into a hillside they are full of earth.
    /// Empty is what its blocks close off from outside, plus everything over
    /// the lowest block it builds in a column (rooms, under an eave, over a
    /// hedge).
    fn rooms(&self) -> Vec<[i32; 3]> {
        let (min, max) = (self.min, self.max);
        let inside = |c: [i32; 3]| (0..3).all(|i| (min[i]..=max[i]).contains(&c[i]));
        let boxed = |c: [i32; 3]| (0..3).all(|i| (min[i] - 1..=max[i] + 1).contains(&c[i]));
        let mut outside: HashSet<[i32; 3]> = HashSet::default();
        let mut queue = vec![[min[0] - 1, min[1] - 1, min[2] - 1]];
        outside.insert(queue[0]);
        while let Some(cell) = queue.pop() {
            for face in crate::geometry::FACES {
                let n = add(cell, face);
                if boxed(n) && !self.filled.contains(&n) && outside.insert(n) {
                    queue.push(n);
                }
            }
        }
        // The lowest thing the design builds in each column: all it leaves
        // unfilled above that is room, porch, eave shadow or sky.
        let mut lowest: HashMap<[i32; 2], i32> = HashMap::default();
        for c in &self.filled {
            let low = lowest.entry([c[0], c[2]]).or_insert(c[1]);
            *low = (*low).min(c[1]);
        }
        let mut rooms = Vec::new();
        for x in min[0]..=max[0] {
            for z in min[2]..=max[2] {
                let low = lowest.get(&[x, z]).copied().unwrap_or(i32::MAX);
                for y in min[1]..=max[1] {
                    let cell = [x, y, z];
                    if !inside(cell) || self.governed.contains(&cell) {
                        continue;
                    }
                    if !outside.contains(&cell) || y > low {
                        rooms.push(cell);
                    }
                }
            }
        }
        rooms
    }

    fn floor_y(&self) -> i32 {
        (self.min[1]..=self.min[1] + 2)
            .max_by_key(|y| self.filled.iter().filter(|c| c[1] == *y).count())
            .unwrap_or(self.min[1])
    }

    /// Rooms become units like any other: cells that must end up empty.
    fn add_rooms(&mut self) {
        let empty = BlockRecord {
            block: EMPTY.into(),
            state: Vec::new(),
            refs: Vec::new(),
            data: Vec::new(),
        };
        if !matches!(
            block_record_plans(vec![empty.clone()]).first(),
            Some(RecordPlan::Air)
        ) {
            return;
        }
        let rooms = self.rooms();
        if rooms.is_empty() {
            return;
        }
        let record = self.records.len() as u32;
        self.records.push(empty);
        self.plans.push(Plan::Air);
        self.costs.push(Vec::new());
        // Not `governed`: a room is where pillars and walkways stand.
        for pos in rooms {
            self.units.push(Unit { pos, record });
        }
    }

    fn finish(&mut self) {
        let floor_y = self.floor_y();
        let plans = &self.plans;
        let rank = |unit: &Unit| match &plans[unit.record as usize] {
            Plan::Air => 0,
            Plan::Build { late: true, .. } => 3,
            Plan::Build {
                attachment: false, ..
            } => 1,
            Plan::Build { .. } => 2,
            Plan::Member | Plan::Unsupported(_) => 4,
        };
        self.units
            .sort_by_key(|u| (u.pos[1], rank(u), u.pos[0], u.pos[2]));
        self.record_index = HashMap::default();
        self.at = self
            .units
            .iter()
            .enumerate()
            .map(|(i, u)| (u.pos, i))
            .collect();
        self.passages = (0..self.units.len())
            .filter(|i| self.doorway(self.units[*i]))
            .collect();
        self.floor = self
            .filled
            .iter()
            .filter(|c| c[1] == floor_y)
            .map(|c| [c[0], c[2]])
            .collect();
        self.complete = true;
    }

    /// Whether a unit fills a way through the walls: two or more cells tall
    /// between sill and lintel, walled on two opposite sides and open on the
    /// others. Built last, so what the walls enclose stays reachable.
    fn doorway(&self, unit: Unit) -> bool {
        let cells = self.cells(unit);
        let [x, _, z] = unit.pos;
        let lo = cells.iter().map(|c| c[1]).min().unwrap_or(0);
        let hi = cells.iter().map(|c| c[1]).max().unwrap_or(0);
        let filled = |c: [i32; 3]| self.filled.contains(&c);
        cells.len() >= 2
            && cells.len() as i32 == hi - lo + 1
            && cells.iter().all(|c| c[0] == x && c[2] == z)
            && filled([x, lo - 1, z])
            && filled([x, hi + 1, z])
            && cells.iter().all(|c| {
                let side = |d: [i32; 3]| filled(add(*c, d));
                let (east, west) = (side([1, 0, 0]), side([-1, 0, 0]));
                let (south, north) = (side([0, 0, 1]), side([0, 0, -1]));
                (east && west && !south && !north) || (south && north && !east && !west)
            })
    }

    /// Whether the unit is a way in: see [`Design::doorway`].
    pub fn passage(&self, unit: usize) -> bool {
        self.passages.contains(&unit)
    }

    /// The unit anchored at `pos`, if any.
    pub fn unit_at(&self, pos: [i32; 3]) -> Option<usize> {
        self.at.get(&pos).copied()
    }

    pub fn plan(&self, unit: Unit) -> &Plan {
        &self.plans[unit.record as usize]
    }

    /// What the unit costs to build from nothing: known from the design
    /// alone, so work still buried in a bank is on the bill before it is dug
    /// out.
    pub fn cost(&self, unit: Unit) -> &[ItemStackData] {
        self.costs
            .get(unit.record as usize)
            .map_or(&[], Vec::as_slice)
    }

    pub fn late(&self, unit: Unit) -> bool {
        matches!(self.plan(unit), Plan::Build { late: true, .. })
    }

    /// Whether the unit is a window pane: see [`Plan::Build`]'s `glazing`.
    pub fn glazing(&self, unit: usize) -> bool {
        matches!(
            self.plan(self.units[unit]),
            Plan::Build { glazing: true, .. }
        )
    }

    pub fn fragile(&self, unit: Unit) -> bool {
        matches!(self.plan(unit), Plan::Build { fragile: true, .. })
    }

    pub fn cells(&self, unit: Unit) -> Vec<[i32; 3]> {
        match self.plan(unit) {
            Plan::Build { footprint, .. } => footprint.iter().map(|o| add(unit.pos, *o)).collect(),
            _ => vec![unit.pos],
        }
    }

    /// Whether the column stands over the design's floor: inside the house.
    pub fn over_floor(&self, cell: [i32; 3]) -> bool {
        self.floor.contains(&[cell[0], cell[2]])
    }

    pub fn contains_column(&self, x: i32, z: i32) -> bool {
        (self.min[0]..=self.max[0]).contains(&x) && (self.min[2]..=self.max[2]).contains(&z)
    }
}

pub fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn turned_size(size: [i32; 3], turns: u8) -> [i32; 3] {
    if turns % 2 == 1 {
        [size[2], size[1], size[0]]
    } else {
        size
    }
}

/// Whether each record's row collides as one whole cube.
/// Per record: whether its block fills the whole cell, and whether it is a
/// thin panel the cell's full height (a pane, bars).
fn record_shapes(records: &[BlockRecord]) -> Vec<(bool, bool)> {
    let ids: Vec<Option<BlockId>> = records.iter().map(|r| resolve_block(&r.block)).collect();
    let known: Vec<BlockId> = ids.iter().flatten().copied().collect();
    let infos = paged(known, block_infos);
    let mut infos = infos.into_iter();
    ids.iter()
        .map(|id| {
            let Some(info) = id.and_then(|_| infos.next().flatten()) else {
                return (false, false);
            };
            let full = matches!(info.collision.as_slice(), [(min, max)] if *min == [0.0; 3] && *max == [1.0; 3]);
            let panel = !info.collision.is_empty()
                && info.collision.iter().all(|(min, max)| {
                    min[1] <= 0.01
                        && (0.99..=1.01).contains(&max[1])
                        && (max[0] - min[0] <= 0.25 || max[2] - min[2] <= 0.25)
                });
            (full, panel)
        })
        .collect()
}

#[cfg(test)]
mod tests;
