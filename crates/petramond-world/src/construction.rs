//! Construction: what paid survival building makes of a portable description
//! of a cell — the items it costs, whether the world already holds it, and
//! the whole-object write that builds it.
//!
//! A [`Record`] is the portable half of a cell (its row, shape state and the
//! data its item carries in). Families own everything shape-specific through
//! the placement seam: the authored intent of a state, the object a record
//! anchors, and the support that object needs. Nothing here names a family.

use std::collections::BTreeMap;

use crate::block::{Block, CellPart, Construction, ShapeNeighborhood, ShapeState};
use crate::item::{variant, ItemStack, ItemType, VariantId};
use crate::mathh::IVec3;
use crate::world::data::WorldData;
use crate::world::placement::{ConstructionWrites, PlacementPlan};

/// What one cell should hold: its row, its shape state, and the cell data a
/// paid item carries into it (the row's `petramond:carry` keys, per part).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub block: Block,
    pub state: ShapeState,
    pub data: BTreeMap<String, Vec<u8>>,
}

/// What a record asks of construction on its own.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// The cell must be empty: clearance, paid in work, never in items.
    Air,
    /// Part of an object anchored at this cell, which builds it.
    Member(IVec3),
    /// An object this cell anchors: the items it costs and every write.
    Unit {
        cost: Vec<ItemStack>,
        writes: PlacementPlan,
    },
    /// Items cannot build this, and why.
    Unsupported(String),
}

/// A record measured against the world.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// The world already holds it.
    Satisfied,
    /// Build it with these writes, paying `missing` (a partly built cell —
    /// one slab of two — pays only its missing parts).
    Place {
        missing: Vec<ItemStack>,
        writes: PlacementPlan,
    },
    /// A block occupies a cell the record needs empty or replaceable.
    Clear {
        at: IVec3,
        block: Block,
    },
    /// A member cell waiting for the object anchored at this cell.
    Pending(IVec3),
    Unsupported(String),
}

impl Record {
    /// The cell at `pos` as the world holds it: every carried key the row (or
    /// each of its parts) declares, and nothing private.
    pub fn at(world: &WorldData, pos: IVec3) -> Self {
        let block = world.physics_block(pos.x, pos.y, pos.z);
        let state = ShapeNeighborhood::shape_state(world, pos);
        let mut data = BTreeMap::new();
        for (part, part_block) in parts_or_whole(world, pos, block) {
            for &key in part_block.carry() {
                let stored = crate::block::part_kv_key(key, part);
                if let Some(value) = world.cell_kv_get(pos.x, pos.y, pos.z, &stored) {
                    data.insert(stored, value.to_vec());
                }
            }
        }
        Self { block, state, data }
    }

    /// A record of `block` in `state` keeping only the cell data its item
    /// carries: private or machine state in `kv` is dropped here, never built.
    pub fn portable(block: Block, state: ShapeState, kv: &BTreeMap<String, Vec<u8>>) -> Self {
        let mut record = Self {
            block,
            state,
            data: BTreeMap::new(),
        };
        let parts = parts_or_whole(
            &Lone {
                pos: IVec3::ZERO,
                record: &record,
            },
            IVec3::ZERO,
            block,
        );
        for (part, part_block) in parts {
            for &key in part_block.carry() {
                let stored = crate::block::part_kv_key(key, part);
                if let Some(value) = kv.get(&stored) {
                    record.data.insert(stored, value.clone());
                }
            }
        }
        record
    }

    /// The record construction actually builds: a row declaring another
    /// construction form (a running machine, a grown plant) builds as that.
    pub fn built(&self) -> Self {
        match self.block.construction() {
            Some(Construction::Form(form)) => Self {
                block: form,
                state: self.state,
                data: self.data.clone(),
            },
            _ => self.clone(),
        }
    }
}

/// The item that pays for one `block`: the row's explicit rule, else the item
/// linked to the row, else the item linked to a sibling orientation row
/// (`rotate_y`), else the item that places it as a wall-facing or flipped
/// variant. Engine-generated creative items never pay.
pub fn paying_item(block: Block) -> Result<ItemType, String> {
    let payable = |item: ItemType| item != ItemType::Air && !item.creative_only();
    match block.construction() {
        Some(Construction::Item(item)) if payable(item) => return Ok(item),
        Some(Construction::Unsupported(reason)) => return Err(reason.to_owned()),
        Some(Construction::Form(form)) if form != block => return paying_item(form),
        _ => {}
    }
    let direct = ItemType::from_block(block);
    if payable(direct) {
        return Ok(direct);
    }
    let mut sibling = block.rotated_row();
    for _ in 0..8 {
        let Some(row) = sibling.filter(|&row| row != block) else {
            break;
        };
        let item = ItemType::from_block(row);
        if payable(item) {
            return Ok(item);
        }
        sibling = row.rotated_row();
    }
    placed_as_variant(block).ok_or_else(|| {
        format!(
            "no item builds {}",
            crate::registry::names()
                .blocks
                .name(block.id())
                .unwrap_or("this block")
        )
    })
}

/// The item whose placement commits `block` as one of its variant rows: a
/// wall-facing sibling, a flipped run, or a sampled decorative variant.
fn placed_as_variant(block: Block) -> Option<ItemType> {
    use std::sync::LazyLock;
    static PLACED_BY: LazyLock<Vec<Option<ItemType>>> = LazyLock::new(|| {
        let mut table = vec![None; Block::all().len()];
        for &item in ItemType::all() {
            if item.creative_only() {
                continue;
            }
            let mut rows: Vec<Block> = item.placement_variants().to_vec();
            if let Some(base) = item.as_block() {
                rows.extend(base.facing_rows().into_iter().flatten());
                rows.extend(base.flipped_row());
            }
            for row in rows {
                table[row.id() as usize].get_or_insert(item);
            }
        }
        table
    });
    PLACED_BY.get(block.id() as usize).copied().flatten()
}

/// The parts of the cell at `pos` holding `block`, or its one whole part.
fn parts_or_whole(nb: &dyn ShapeNeighborhood, pos: IVec3, block: Block) -> Vec<(CellPart, Block)> {
    let k = block.shape_kind_def();
    k.sim
        .parts(&k.params, nb, pos, block)
        .unwrap_or_else(|| vec![(0, block)])
}

/// A neighbourhood holding one record at one cell and nothing anywhere else:
/// what a family answers about a record before it is in any world.
struct Lone<'a> {
    pos: IVec3,
    record: &'a Record,
}

impl ShapeNeighborhood for Lone<'_> {
    fn block(&self, pos: IVec3) -> Block {
        if pos == self.pos {
            self.record.block
        } else {
            Block::Air
        }
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        if pos == self.pos {
            self.record.state
        } else {
            ShapeState::NONE
        }
    }
}

/// What `record` at `pos` asks of construction.
pub fn plan(record: &Record, pos: IVec3) -> Plan {
    let record = record.built();
    let block = record.block;
    if block == Block::Air {
        return Plan::Air;
    }
    if block.is_fluid() {
        return Plan::Unsupported("fluids are not built from items".into());
    }
    let k = block.shape_kind_def();
    let writes = match k.placement.construction_writes(block, record.state, pos) {
        ConstructionWrites::Member(anchor) => return Plan::Member(anchor),
        ConstructionWrites::Anchor(writes) => writes,
    };
    let parts = parts_or_whole(
        &Lone {
            pos,
            record: &record,
        },
        pos,
        block,
    );
    match cost_of(&record, &parts) {
        Ok(cost) => Plan::Unit { cost, writes },
        Err(reason) => Plan::Unsupported(reason),
    }
}

/// The items `parts` of `record` cost, each carrying that part's data; equal
/// stacks merge.
fn cost_of(record: &Record, parts: &[(CellPart, Block)]) -> Result<Vec<ItemStack>, String> {
    let mut cost: Vec<ItemStack> = Vec::new();
    for &(part, part_block) in parts {
        let item = paying_item(part_block)?;
        let variant = part_variant(record, part, part_block)?;
        match cost
            .iter_mut()
            .find(|s| s.item == item && s.variant == variant)
        {
            Some(stack) => stack.count += 1,
            None => cost.push(ItemStack::with_variant(item, 1, variant)),
        }
    }
    Ok(cost)
}

/// The instance data a part's paying item must carry: the record's entries
/// for that part under the part block's carried keys.
fn part_variant(record: &Record, part: CellPart, part_block: Block) -> Result<VariantId, String> {
    let mut map = variant::VariantMap::new();
    for &key in part_block.carry() {
        if let Some(value) = record.data.get(&crate::block::part_kv_key(key, part)) {
            map.insert(key.to_owned(), value.clone());
        }
    }
    if map.is_empty() {
        return Ok(VariantId::NONE);
    }
    variant::intern(&map).ok_or_else(|| "the carried item data cannot be represented".into())
}

/// Measure `record` at `pos` against `world`. The caller gates terrain
/// finality: every cell of the object must be stream-final.
pub fn status(world: &WorldData, pos: IVec3, record: &Record) -> Status {
    let want = record.built();
    match plan(&want, pos) {
        Plan::Unsupported(reason) => Status::Unsupported(reason),
        Plan::Air => match world.physics_block(pos.x, pos.y, pos.z) {
            Block::Air => Status::Satisfied,
            block => Status::Clear { at: pos, block },
        },
        Plan::Member(anchor) => {
            let k = want.block.shape_kind_def();
            let built = k.placement.member_state(want.block, want.state, pos);
            if holds(world, pos, want.block, built) {
                Status::Satisfied
            } else {
                match world.physics_block(pos.x, pos.y, pos.z) {
                    block if block.is_replaceable() => Status::Pending(anchor),
                    block => Status::Clear { at: pos, block },
                }
            }
        }
        Plan::Unit { cost, writes } => unit_status(world, pos, &want, cost, writes),
    }
}

fn unit_status(
    world: &WorldData,
    pos: IVec3,
    want: &Record,
    cost: Vec<ItemStack>,
    writes: PlacementPlan,
) -> Status {
    let built = writes.writes.iter().all(|w| {
        let cell = Record {
            block: w.block,
            state: w.state,
            data: if w.cell == writes.anchor {
                want.data.clone()
            } else {
                BTreeMap::new()
            },
        };
        cell_matches(world, w.cell, &cell, w.cell == writes.anchor)
    });
    if built {
        return Status::Satisfied;
    }
    if let Some(partial) = partial_parts(world, pos, want, &writes) {
        return partial;
    }
    for w in &writes.writes {
        let block = world.physics_block(w.cell.x, w.cell.y, w.cell.z);
        if !block.is_replaceable() {
            return Status::Clear { at: w.cell, block };
        }
    }
    Status::Place {
        missing: cost,
        writes,
    }
}

/// A single-cell record of several parts over a cell already holding some
/// of them (and nothing else): build the rest, keeping what stands.
fn partial_parts(
    world: &WorldData,
    pos: IVec3,
    want: &Record,
    writes: &PlacementPlan,
) -> Option<Status> {
    if writes.writes.len() != 1 {
        return None;
    }
    let k = want.block.shape_kind_def();
    let want_parts = k
        .sim
        .parts(&k.params, &Lone { pos, record: want }, pos, want.block)?;
    let held = world.physics_block(pos.x, pos.y, pos.z);
    if held == Block::Air || held.shape_kind_def().family != k.family {
        return None;
    }
    let have_parts = world.cell_parts(pos)?;
    let have = Record::at(world, pos);
    let carried = |record: &Record, part: CellPart, block: Block| {
        block
            .carry()
            .iter()
            .map(|&key| {
                record
                    .data
                    .get(&crate::block::part_kv_key(key, part))
                    .cloned()
            })
            .collect::<Vec<_>>()
    };
    let present = |&(part, block): &(CellPart, Block)| {
        have_parts.iter().any(|&(p, b)| p == part && b == block)
            && carried(&have, part, block) == carried(want, part, block)
    };
    if !have_parts.iter().all(|h| want_parts.contains(h)) || !want_parts.iter().any(present) {
        return None;
    }
    let missing: Vec<_> = want_parts.iter().copied().filter(|p| !present(p)).collect();
    // What stands must be the record with some parts yet to lay: the same
    // parts lying another way are in the way, not a start.
    let standing: Vec<CellPart> = have_parts.iter().map(|&(part, _)| part).collect();
    let (kept_block, kept_state) = k
        .sim
        .keeping_parts(&k.params, want.block, want.state, &standing)?;
    if missing.is_empty() || !holds(world, pos, kept_block, kept_state) {
        return None;
    }
    let cost = cost_of(want, &missing).ok()?;
    let mut writes = PlacementPlan::single_part(
        pos,
        want.block,
        k.placement.authored_state(want.block, want.state),
        missing.first().map_or(0, |&(part, _)| part),
        true,
    );
    writes.anchor = pos;
    Some(Status::Place {
        missing: cost,
        writes,
    })
}

/// Whether the cell at `pos` already holds `want`: the same row (or a row
/// declaring `want`'s row as its construction form — a machine that has since
/// been lit), the same authored intent once `want` is refined against the
/// cell's actual neighbours, and — at an object's anchor — the same carried
/// data.
fn cell_matches(world: &WorldData, pos: IVec3, want: &Record, with_data: bool) -> bool {
    holds(world, pos, want.block, want.state)
        && (!with_data || Record::at(world, pos).data == want.data)
}

/// Whether the cell at `pos` of `nb` holds `block` with `state`'s authored
/// intent, both read against the neighbours `nb` gives them.
fn holds(nb: &dyn ShapeNeighborhood, pos: IVec3, block: Block, state: ShapeState) -> bool {
    let held = nb.block(pos);
    if held != block && held.construction() != Some(Construction::Form(block)) {
        return false;
    }
    let k = block.shape_kind_def();
    let intent = |b: Block, s: ShapeState| {
        k.placement
            .authored_state(b, k.sim.refine_state(&k.params, nb, pos, b, s))
    };
    intent(held, nb.shape_state(pos)) == intent(block, state)
}

/// The item a click lays to build `part` of `record`, carrying that part's
/// data.
pub fn part_stack(record: &Record, part: CellPart, part_block: Block) -> Option<ItemStack> {
    cost_of(record, &[(part, part_block)]).ok()?.pop()
}

/// Whether `step` — what one click of `paid` would write — is a click's worth
/// of the object `want` builds for `record`: the whole object as recorded, or
/// one more of a cell's parts (a slab of two) with nothing the record lacks.
pub fn advances(
    world: &WorldData,
    step: &PlacementPlan,
    want: &PlacementPlan,
    record: &Record,
    paid: &ItemStack,
) -> bool {
    let record = record.built();
    let after = Planned {
        world,
        writes: step,
    };
    let cell_of = |plan: &'_ PlacementPlan, cell: IVec3| plan.writes.iter().any(|w| w.cell == cell);
    if step.anchor != want.anchor || !step.writes.iter().all(|w| cell_of(want, w.cell)) {
        return false;
    }
    let laid = after.block(want.anchor);
    let k = laid.shape_kind_def();
    let have_parts = k.sim.parts(&k.params, &after, want.anchor, laid);
    // The click filled one part, and is paid with that part's item.
    let filled = step.anchor_part();
    let filled_block = match &have_parts {
        Some(parts) => parts.iter().find(|(part, _)| *part == filled).map(|p| p.1),
        None => Some(record.block),
    };
    if filled_block
        .and_then(|block| part_stack(&record, filled, block))
        .as_ref()
        != Some(paid)
    {
        return false;
    }
    let whole = want
        .writes
        .iter()
        .all(|w| cell_of(step, w.cell) && holds(&after, w.cell, w.block, w.state));
    // Short of the whole, some of the record's parts: the cell stands as the
    // record would with only those laid.
    whole
        || have_parts.is_some_and(|have| {
            let keep: Vec<CellPart> = have.iter().map(|&(part, _)| part).collect();
            let k = record.block.shape_kind_def();
            k.sim
                .keeping_parts(&k.params, record.block, record.state, &keep)
                .is_some_and(|(block, state)| holds(&after, want.anchor, block, state))
        })
}

/// Whether some face of the object `writes` builds rests against a block
/// outside it — the face a placement clicks against. Replaceable neighbours
/// (air, plants, fluids) offer none.
pub fn has_placement_face(world: &WorldData, writes: &PlacementPlan) -> bool {
    const FACES: [IVec3; 6] = [
        IVec3::X,
        IVec3::NEG_X,
        IVec3::Y,
        IVec3::NEG_Y,
        IVec3::Z,
        IVec3::NEG_Z,
    ];
    writes.writes.iter().any(|w| {
        FACES.iter().any(|&face| {
            let n = w.cell + face;
            !writes.writes.iter().any(|o| o.cell == n)
                && !world.physics_block(n.x, n.y, n.z).is_replaceable()
        })
    })
}

/// The world with a plan's writes laid over it.
struct Planned<'a> {
    world: &'a WorldData,
    writes: &'a PlacementPlan,
}

impl ShapeNeighborhood for Planned<'_> {
    fn block(&self, pos: IVec3) -> Block {
        match self.writes.writes.iter().find(|w| w.cell == pos) {
            Some(w) => w.block,
            None => self.world.physics_block(pos.x, pos.y, pos.z),
        }
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        match self.writes.writes.iter().find(|w| w.cell == pos) {
            Some(w) => w.state,
            None => ShapeNeighborhood::shape_state(self.world, pos),
        }
    }

    fn baked(&self, pos: IVec3) -> Option<&[crate::block::ShapeRenderBox]> {
        if self.writes.writes.iter().any(|w| w.cell == pos) {
            None
        } else {
            self.world.baked(pos)
        }
    }

    fn baked_collision(&self, pos: IVec3) -> Option<&'static [crate::block::Aabb]> {
        if self.writes.writes.iter().any(|w| w.cell == pos) {
            None
        } else {
            self.world.baked_collision(pos)
        }
    }
}
