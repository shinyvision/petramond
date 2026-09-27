use petramond_world::block::{Block, MeshEmitter};
use petramond_world::content::stage::BLOCK_VIEWS;
use petramond_world::content::{ContentRegistry, Slot};

pub(super) const SKIP: u8 = 1 << 0;
pub(super) const FAST_CUBE: u8 = 1 << 1;
pub(super) const FLUID: u8 = 1 << 2;

pub(super) const PAD_OPAQUE: u8 = 1 << 0;
pub(super) const PAD_SLAB: u8 = 1 << 1;
pub(super) const PAD_SEALS: u8 = 1 << 2;
pub(super) const PAD_OPAQUE_FLUID: u8 = 1 << 3;

pub struct MeshRegistry {
    cell: Box<[u8]>,
    pad: Box<[u8]>,
    emitters: Box<[MeshEmitter]>,
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
