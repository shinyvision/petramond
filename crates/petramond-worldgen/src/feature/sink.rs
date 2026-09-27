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
    let mut sink = SectionSink::new(section);
    for &([x, y, z], id) in writes {
        sink.set(IVec3::new(x, y, z), Block(id));
    }
}

pub fn apply_gen_plan(section: &mut Section, plan: &crate::hooks::GenerationPlan) {
    let modified = section.modified;
    apply_gen_writes(section, &plan.blocks);
    for feature in &plan.features {
        feature.apply(section);
    }
    let (origin, size) = section.world_box();
    let clip = petramond_world::structure::Bounds {
        min: origin,
        max: origin + size - IVec3::ONE,
    };
    for placement in &plan.structures {
        placement.visit(clip, |pos, cell| {
            let local = pos - origin;
            let (x, y, z) = (local.x as usize, local.y as usize, local.z as usize);
            section.set_block(x, y, z, cell.block);
            section.set_cell_state(x, y, z, cell.state);
            for (key, value) in &cell.data {
                section.cell_kv_set(x, y, z, key.clone(), value.clone());
            }
        });
    }
    section.modified = modified;
}
