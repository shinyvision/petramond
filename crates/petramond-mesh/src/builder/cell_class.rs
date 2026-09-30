use petramond_world::block::{Block, MeshEmitter};
use petramond_world::content::stage::BLOCK_VIEWS;
use petramond_world::content::{ContentRegistry, Slot};
use petramond_world::tile::{Tile, TileTint, VariationSelect};

pub(super) const SKIP: u8 = 1 << 0;
pub(super) const FAST_CUBE: u8 = 1 << 1;
pub(super) const FLUID: u8 = 1 << 2;

pub(super) const PAD_OPAQUE: u8 = 1 << 0;
pub(super) const PAD_SLAB: u8 = 1 << 1;
pub(super) const PAD_SEALS: u8 = 1 << 2;
pub(super) const PAD_OPAQUE_FLUID: u8 = 1 << 3;
/// The pad class plus the two bits the face-lighting ring gather asks per ring cell.
pub(super) const RING_OCCLUDES_AO: u8 = 1 << 4;
pub(super) const RING_BOX_SHAPE: u8 = 1 << 5;

pub struct MeshRegistry {
    cell: Box<[u8]>,
    pad: Box<[u8]>,
    emitters: Box<[MeshEmitter]>,
    cubes: Box<[CubeRow]>,
    contained: Box<[Option<Block>]>,
    tiles: Box<[TileMeta]>,
    ring: Box<[u8]>,
}

/// Everything the cube emitter reads off a block ROW, one dense record per id, so a visited
/// cube cell costs one table read instead of a registry row load per accessor per face.
#[derive(Copy, Clone)]
pub(super) struct CubeRow {
    pub(super) tiles: [Tile; 3],
    pub(super) front: Option<Tile>,
    pub(super) covered_side: Option<Tile>,
    /// `(base, overlay)` of a `side_overlay` row.
    pub(super) side_overlay: Option<(Tile, Tile)>,
    pub(super) uv_turns: [u8; 3],
    bits: u8,
}

pub(super) const ROW_LOG: u8 = 1 << 0;
pub(super) const ROW_OPAQUE: u8 = 1 << 1;
pub(super) const ROW_TRANSLUCENT: u8 = 1 << 2;
pub(super) const ROW_LEAVES: u8 = 1 << 3;
pub(super) const ROW_CANOPY: u8 = 1 << 4;
pub(super) const ROW_MERGES_SELF: u8 = 1 << 5;

impl CubeRow {
    #[inline]
    pub(super) fn has(self, bit: u8) -> bool {
        self.bits & bit != 0
    }

    fn of(block: Block) -> Self {
        let mut bits = 0;
        for (on, bit) in [
            (block.is_axial(), ROW_LOG),
            (block.is_opaque(), ROW_OPAQUE),
            (block.is_translucent(), ROW_TRANSLUCENT),
            (block.is_leaves(), ROW_LEAVES),
            (block.is_canopy(), ROW_CANOPY),
            (block.merges_with_self(), ROW_MERGES_SELF),
        ] {
            if on {
                bits |= bit;
            }
        }
        Self {
            tiles: block.tiles(),
            front: block.front_tile(),
            covered_side: block.covered_side(),
            side_overlay: block.side_overlay().map(|so| (so.base, so.overlay)),
            uv_turns: block.uv_turns(),
            bits,
        }
    }
}

/// The per-tile facts a face read through the atlas registry: the face-variation count
/// (0 = the tile does not vary per face) and the world tint kind.
#[derive(Copy, Clone)]
pub(super) struct TileMeta {
    pub(super) face_variation: u16,
    pub(super) world_tint: Option<TileTint>,
}

impl TileMeta {
    fn of(tile: Tile) -> Self {
        Self {
            face_variation: match tile.variation_select() {
                Some(VariationSelect::Face) => tile.variation_count() as u16,
                _ => 0,
            },
            world_tint: tile.world_tint(),
        }
    }
}

static MESH_REGISTRY: Slot<MeshRegistry> =
    Slot::new("mesh block rows", &[BLOCK_VIEWS], derive_mesh_registry);

fn derive_mesh_registry(_: &ContentRegistry) -> Result<MeshRegistry, String> {
    Ok(MeshRegistry::from_blocks(Block::all()))
}

impl MeshRegistry {
    pub fn from_blocks(blocks: &[Block]) -> Self {
        assert!(
            blocks.first() == Some(&Block::Air),
            "a mesh registry's row 0 is air"
        );
        Self {
            cell: blocks.iter().map(|&b| cell_class(b)).collect(),
            pad: blocks.iter().map(|&b| pad_class(b)).collect(),
            emitters: blocks.iter().map(|b| b.mesh_emitter()).collect(),
            cubes: blocks.iter().map(|&b| CubeRow::of(b)).collect(),
            contained: blocks.iter().map(|b| b.contained_fluid()).collect(),
            tiles: Tile::all().map(TileMeta::of).collect(),
            ring: blocks.iter().map(|&b| ring_class(b)).collect(),
        }
    }

    #[inline]
    pub(super) fn ring_class(&self, id: u16) -> u8 {
        class_of(&self.ring, id)
    }

    #[inline]
    pub(super) fn cube_row(&self, id: u16) -> CubeRow {
        match self.cubes.get(id as usize) {
            Some(row) => *row,
            None => self.cubes[0],
        }
    }

    #[inline]
    pub(super) fn contained_fluid(&self, id: u16) -> Option<Block> {
        self.contained.get(id as usize).copied().flatten()
    }

    #[inline]
    pub(super) fn tile_meta(&self, tile: Tile) -> TileMeta {
        self.tiles[tile.index()]
    }

    #[inline]
    pub(super) fn world_tint(&self, tile: Tile) -> Option<TileTint> {
        self.tile_meta(tile).world_tint
    }

    /// [`Tile::face_variation`] off the dense table.
    #[inline]
    pub(super) fn face_variation(&self, tile: Tile, cell: [i32; 3], normal: u32) -> Tile {
        let n = self.tile_meta(tile).face_variation;
        if n == 0 {
            tile
        } else {
            tile.variant((petramond_world::tile::spatial_hash(cell, normal) % u32::from(n)) as u16)
        }
    }

    pub fn global() -> &'static MeshRegistry {
        MESH_REGISTRY.current()
    }

    #[inline]
    pub(super) fn cell_class(&self, id: u16) -> u8 {
        class_of(&self.cell, id)
    }

    #[inline]
    pub(super) fn pad_class(&self, id: u16) -> u8 {
        class_of(&self.pad, id)
    }

    #[inline]
    pub(super) fn emitter(&self, id: u16) -> MeshEmitter {
        self.emitters
            .get(id as usize)
            .copied()
            .unwrap_or(self.emitters[0])
    }
}

fn pad_class(block: Block) -> u8 {
    let mut c = 0;
    if block.is_opaque() {
        c |= PAD_OPAQUE;
    }
    if block.is_slab() {
        c |= PAD_SLAB;
    }
    if block.fluid_def().is_some_and(|def| def.medium.is_opaque()) {
        c |= PAD_OPAQUE_FLUID;
    }
    let seals_by_shape =
        block.has_box_shape() && !block.is_transparent() && !block.is_translucent();
    if block != Block::Air && (seals_by_shape || block.is_snow_bedded()) {
        c |= PAD_SEALS;
    }
    c
}

fn ring_class(block: Block) -> u8 {
    let mut c = pad_class(block);
    if block.occludes_ao() {
        c |= RING_OCCLUDES_AO;
    }
    if block.has_box_shape() {
        c |= RING_BOX_SHAPE;
    }
    c
}

fn cell_class(block: Block) -> u8 {
    let mut c = if block == Block::Air
        || block.flags().invisible()
        || block.mesh_emitter() == MeshEmitter::Nothing
    {
        SKIP
    } else {
        0
    };
    if c & SKIP == 0 && fast_cube_candidate(block) {
        c |= FAST_CUBE;
    }
    if block.is_fluid() {
        c |= FLUID;
    }
    c
}

fn fast_cube_candidate(block: Block) -> bool {
    !block.is_fluid()
        && !block.merges_with_self()
        && !block.is_translucent()
        && block.mesh_emitter() == MeshEmitter::Cube
}

#[inline]
fn class_of(table: &[u8], id: u16) -> u8 {
    table.get(id as usize).copied().unwrap_or(table[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_answers_for_exactly_the_blocks_it_was_built_from() {
        let all = MeshRegistry::from_blocks(Block::all());
        let air_only = MeshRegistry::from_blocks(&[Block::Air]);
        for &block in Block::all() {
            let id = block.id();
            assert_eq!(all.cell_class(id), cell_class(block), "{block:?}");
            assert_eq!(all.pad_class(id), pad_class(block), "{block:?}");
            assert_eq!(all.emitter(id), block.mesh_emitter(), "{block:?}");
            assert_eq!(air_only.cell_class(id), cell_class(Block::Air));
            assert_eq!(air_only.pad_class(id), pad_class(Block::Air));
        }
        assert_ne!(all.cell_class(0) & SKIP, 0, "air is skipped");
        assert_eq!(all.cell_class(u16::MAX), all.cell_class(0));
    }
}
