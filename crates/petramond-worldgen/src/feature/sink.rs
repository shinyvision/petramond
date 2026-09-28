use petramond_world::block::Block;
use petramond_world::chunk::SECTION_SIZE;
#[cfg(test)]
use petramond_world::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use petramond_world::mathh::IVec3;
use petramond_world::section::Section;

#[cfg(test)]
mod tests;

pub trait VoxelSink {
    fn get(&self, p: IVec3) -> Block;
    fn set(&mut self, p: IVec3, b: Block);
    fn place(&mut self, p: IVec3, b: Block, rule: PlacementRule) {
        if matches!(rule, PlacementRule::Always) || rule.allows(self.get(p)) {
            self.set(p, b);
        }
    }
}

#[derive(Clone, Copy)]
pub enum PlacementRule {
    Always,
    Leaf,
    Branch,
    Litter,
    Replace(&'static [Block]),
}

impl PlacementRule {
    fn allows(self, block: Block) -> bool {
        match self {
            Self::Always => true,
            Self::Leaf => {
                block == Block::Air
                    || block == Block::Water
                    || block.is_fragile()
                    || block.is_snow_cover()
            }
            Self::Branch => block == Block::Air || block == Block::Water || block.is_leaves(),
            Self::Litter => block == Block::Air || block == Block::Water || block.is_snow_cover(),
            Self::Replace(hosts) => hosts.contains(&block),
        }
    }
}

pub trait SinkTarget {
    fn world_box(&self) -> (IVec3, IVec3);
    fn block(&self, x: usize, y: usize, z: usize) -> Block;
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u16);
}

pub struct ClippedSink<'a, T: SinkTarget> {
    target: &'a mut T,
    origin: IVec3,
    size: IVec3,
}

impl<'a, T: SinkTarget> ClippedSink<'a, T> {
    pub fn new(target: &'a mut T) -> Self {
        let (origin, size) = target.world_box();
        Self {
            target,
            origin,
            size,
        }
    }

    #[inline]
    fn local(&self, p: IVec3) -> Option<(usize, usize, usize)> {
        let l = p - self.origin;
        if l.cmpge(IVec3::ZERO).all() && l.cmplt(self.size).all() {
            Some((l.x as usize, l.y as usize, l.z as usize))
        } else {
            None
        }
    }
}

impl<T: SinkTarget> VoxelSink for ClippedSink<'_, T> {
    #[inline]
    fn get(&self, p: IVec3) -> Block {
        match self.local(p) {
            Some((x, y, z)) => self.target.block(x, y, z),
            None => Block::Air,
        }
    }
    #[inline]
    fn set(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            self.target.set_block_raw(x, y, z, b.id());
        }
    }
}

#[cfg(test)]
impl SinkTarget for Chunk {
    fn world_box(&self) -> (IVec3, IVec3) {
        let (ox, oz) = self.chunk_origin_world();
        let size = IVec3::new(CHUNK_SX as i32, CHUNK_SY as i32, CHUNK_SZ as i32);
        (IVec3::new(ox, 0, oz), size)
    }
    #[inline]
    fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Chunk::block(self, x, y, z)
    }
    #[inline]
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u16) {
        Chunk::set_block_raw(self, x, y, z, id);
    }
}

impl SinkTarget for Section {
    fn world_box(&self) -> (IVec3, IVec3) {
        let (ox, oy, oz) = self.origin_world();
        (IVec3::new(ox, oy, oz), IVec3::splat(SECTION_SIZE as i32))
    }
    #[inline]
    fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Section::block(self, x, y, z)
    }
    #[inline]
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u16) {
        Section::set_block_raw(self, x, y, z, id);
    }
}

#[cfg(test)]
pub type ChunkSink<'a> = ClippedSink<'a, Chunk>;

pub type SectionSink<'a> = ClippedSink<'a, Section>;

pub fn apply_gen_writes(section: &mut Section, writes: &[([i32; 3], u16)]) {
    let (origin, _) = section.world_box();
    section.set_blocks_raw(writes.iter().filter_map(|&(pos, id)| {
        let d = IVec3::from(pos) - origin;
        ((d.x as u32 | d.y as u32 | d.z as u32) < SECTION_SIZE as u32)
            .then_some(([d.x as usize, d.y as usize, d.z as usize], id))
    }));
}

pub fn apply_gen_plan(section: &mut Section, plan: &crate::hooks::GenerationPlan) {
    let modified = section.modified;
    let (origin, size) = section.world_box();
    for fill in &plan.fills {
        let lo = IVec3::from(fill.min).max(origin) - origin;
        let hi = IVec3::from(fill.max).min(origin + size - IVec3::ONE) - origin;
        if lo.cmple(hi).all() {
            let local = |v: IVec3| v.to_array().map(|c| c as usize);
            section.fill_box(local(lo), local(hi), fill.block.0);
        }
    }
    apply_gen_writes(section, &plan.blocks);
    let clip = petramond_world::structure::Bounds {
        min: origin,
        max: origin + size - IVec3::ONE,
    };
    let local = |pos: IVec3| {
        let d = pos - origin;
        ((d.x as u32 | d.y as u32 | d.z as u32) < SECTION_SIZE as u32).then_some([
            d.x as usize,
            d.y as usize,
            d.z as usize,
        ])
    };
    section.set_blocks_raw(
        plan.authored
            .iter()
            .filter_map(|cell| Some((local(cell.pos)?, cell.block.id()))),
    );
    let mut states = Vec::new();
    for cell in &plan.authored {
        if cell.state.is_empty() && cell.data.is_empty() {
            continue;
        }
        if let Some([x, y, z]) = local(cell.pos) {
            if !cell.state.is_empty() {
                states.push((
                    petramond_world::chunk::section_idx(x, y, z) as u16,
                    cell.state,
                ));
            }
            for (key, value) in &cell.data {
                section.cell_kv_set(x, y, z, key.clone(), value.clone());
            }
        }
    }
    section.extend_cell_states(states);
    for feature in &plan.features {
        feature.apply(section);
    }
    for placement in &plan.structures {
        placement.visit(clip, |pos, cell| {
            write_authored(section, pos - origin, cell)
        });
    }
    section.modified = modified;
}

fn write_authored(section: &mut Section, local: IVec3, cell: &petramond_world::structure::Cell) {
    let (x, y, z) = (local.x as usize, local.y as usize, local.z as usize);
    section.set_block(x, y, z, cell.block);
    if !cell.state.is_empty() {
        section.set_cell_state(x, y, z, cell.state);
    }
    for (key, value) in &cell.data {
        section.cell_kv_set(x, y, z, key.clone(), value.clone());
    }
}
