use std::collections::BTreeMap;

use crate::block::{Block, CellPart, Construction, ShapeNeighborhood, ShapeState};
use crate::item::{variant, ItemStack, ItemType, VariantId};
use crate::mathh::IVec3;
use crate::world::data::WorldData;
use crate::world::placement::{ConstructionWrites, PlacementPlan};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub block: Block,
    pub state: ShapeState,
    pub data: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Air,
    Member(IVec3),
    Unit {
        cost: Vec<ItemStack>,
        writes: PlacementPlan,
    },
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Satisfied,
    Place {
        missing: Vec<ItemStack>,
        writes: PlacementPlan,
    },
    Clear {
        at: IVec3,
        block: Block,
    },
    Pending(IVec3),
    Unsupported(String),
}

impl Record {
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

fn placed_as_variant(block: Block) -> Option<ItemType> {
    PLACED_BY
        .current()
        .get(block.id() as usize)
        .copied()
        .flatten()
}

pub(crate) static PLACED_BY: crate::content::Slot<Vec<Option<ItemType>>> =
    crate::content::Slot::new(
        crate::content::stage::CONSTRUCTION,
        &[crate::content::stage::ITEMS],
        derive_placed_by,
    );

fn derive_placed_by(_: &crate::content::ContentRegistry) -> Result<Vec<Option<ItemType>>, String> {
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
    Ok(table)
}

fn parts_or_whole(nb: &dyn ShapeNeighborhood, pos: IVec3, block: Block) -> Vec<(CellPart, Block)> {
    let k = block.shape_kind_def();
    k.sim
        .parts(&k.params, nb, pos, block)
        .unwrap_or_else(|| vec![(0, block)])
}

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
    variant::intern(&map).map_err(|e| format!("the carried item data cannot be represented: {e}"))
}

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

fn cell_matches(world: &WorldData, pos: IVec3, want: &Record, with_data: bool) -> bool {
    holds(world, pos, want.block, want.state)
        && (!with_data || Record::at(world, pos).data == want.data)
}

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

pub fn part_stack(record: &Record, part: CellPart, part_block: Block) -> Option<ItemStack> {
    cost_of(record, &[(part, part_block)]).ok()?.pop()
}

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
    whole
        || have_parts.is_some_and(|have| {
            let keep: Vec<CellPart> = have.iter().map(|&(part, _)| part).collect();
            let k = record.block.shape_kind_def();
            k.sim
                .keeping_parts(&k.params, record.block, record.state, &keep)
                .is_some_and(|(block, state)| holds(&after, want.anchor, block, state))
        })
}

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
