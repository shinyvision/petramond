use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_SIZE};
use crate::mathh::IVec3;
use crate::section::Section;

/// A destination a feature paints voxels into. Abstracting WHERE the writes land
/// lets the SAME `Feature` / placer code drive two callers: worldgen, which writes
/// into one [`Chunk`] clipped to its footprint ([`ChunkSink`]), and runtime sapling
/// growth, which writes into the live `World` through a validating overlay (see
/// `world::sapling`). `get` returns the sink's CURRENT occupant so the overwrite
/// predicates on [`FeatureCtx`] see a feature's own earlier writes; it reads `Air`
/// for any cell the sink can't address.
pub trait VoxelSink {
    fn get(&self, p: IVec3) -> Block;
    fn set(&mut self, p: IVec3, b: Block);
}

/// Bulk voxel storage a [`ClippedSink`] clips into: a world-anchored writable
/// box plus raw local-index accessors.
pub trait SinkTarget {
    /// `(min world corner, size in blocks)` of the writable footprint.
    fn world_box(&self) -> (IVec3, IVec3);
    fn block(&self, x: usize, y: usize, z: usize) -> Block;
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8);
}

/// Worldgen voxel sink: writes into one [`SinkTarget`], in WORLD coords clipped to
/// the target's own footprint. Out-of-footprint writes are dropped and
/// out-of-footprint reads return `Air`. That clipping IS the seam mechanism:
/// every retained write predicates only on the cell it writes (`set_leaf`/
/// `set_branch` read `get(p)` at the same `p`), never a neighbour, so a feature
/// rooted anywhere materialises its overlapping voxels identically whether they
/// land in the owner target or a neighbour — seam-consistent cross-boundary
/// features with no shared buffer.
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

    /// Map a world position to in-footprint local indices, or `None` if outside.
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
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
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
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        Section::set_block_raw(self, x, y, z, id);
    }
}

/// [`ClippedSink`] over one [`Chunk`]'s `[0,16)×[0,256)×[0,16)` footprint —
/// seam-consistent cross-chunk features with no shared buffer.
pub type ChunkSink<'a> = ClippedSink<'a, Chunk>;

/// [`ClippedSink`] over one 16³ [`Section`] for the cubic path — the same seam
/// mechanism in 3D, so a feature materialises its in-section voxels identically
/// whether the section is generated alone or as part of a whole column, across
/// VERTICAL seams as well as horizontal ones.
pub type SectionSink<'a> = ClippedSink<'a, Section>;

/// Apply a mod worldgen hook's write list (world position, registered block
/// id) to one section through the SAME clipping sink engine features use —
/// out-of-section writes drop, in-section writes go through the counted
/// setter. That clip is the mod-feature seam mechanism: every section
/// materialises exactly its own slice of a cross-boundary feature.
pub(crate) fn apply_gen_writes(section: &mut Section, writes: &[([i32; 3], u8)]) {
    let mut sink = SectionSink::new(section);
    for &([x, y, z], id) in writes {
        sink.set(IVec3::new(x, y, z), Block(id));
    }
}
