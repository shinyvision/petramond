//! The chunk mesher's per-block-id DISPATCH tables: dense per-id lookups that
//! answer every "which emitter does this cell use" question the cell scan
//! asks, baked once from the block rows.
//!
//! The scan visits 4096 cells per section; re-deriving the answer per cell
//! from the registry would pay a big-table shape-kind load and several
//! lazy-static checks each time. Every answer depends only on the block id, so
//! it is read off the row once — the emitter is the row's own declared
//! [`MeshEmitter`] (its shape family's facet), never a list of named blocks or
//! families kept here.
//!
//! The tables are a value, [`MeshRegistry`], built from a block list and
//! handed to the mesher with every build. The default table is kept in a
//! [`Slot`] on the current content registry, so a worker pinned to another
//! world's mod set gets that world's block rows.

use petramond_world::block::{Block, MeshEmitter};
use petramond_world::content::stage::BLOCK_VIEWS;
use petramond_world::content::{ContentRegistry, Slot};

/// Air, invisible rows, and every row drawn outside the chunk mesh by its
/// animated block model emit nothing here.
pub(super) const SKIP: u8 = 1 << 0;
/// Eligible for the exposure-mask cube fast path.
pub(super) const FAST_CUBE: u8 = 1 << 1;
/// Meshes through the fluid emitter (checked before the row's emitter).
pub(super) const FLUID: u8 = 1 << 2;

/// A SECOND dense byte, for the exposure-mask build's pad scan (which asks
/// different questions of every one of 5832 pad cells than the cell scan asks
/// of the section's 4096).
pub(super) const PAD_OPAQUE: u8 = 1 << 0;
pub(super) const PAD_SLAB: u8 = 1 << 1;
/// The only cells that can seal the boundary beneath them — non-air box
/// shapes that are not see-through, and `snow_bedded` rows (the blanket they
/// stand in seals like the snow layer it stands in for, even under a
/// transparent plant). Everything else (air, water, plain plants, leaves,
/// plain cubes) is rejected by `boxset::cell_seals_face` anyway, and asking
/// it costs a world read plus a big-table shape-kind load.
pub(super) const PAD_SEALS: u8 = 1 << 2;
/// A fluid whose medium is opaque: a FULL cell of it covers the faces behind
/// it (the fill check needs the cell's meta, so this bit only nominates).
pub(super) const PAD_OPAQUE_FLUID: u8 = 1 << 3;

/// The mesher's dense per-block-id dispatch tables, baked once from a block
/// list. Ids past the list read as air (row 0), exactly where
/// `Block::from_id` degrades a raw id to air.
pub struct MeshRegistry {
    /// Cell-scan class bits ([`SKIP`], [`FAST_CUBE`], [`FLUID`]).
    cell: Box<[u8]>,
    /// Exposure-mask pad-scan class bits ([`PAD_OPAQUE`] and friends).
    pad: Box<[u8]>,
    /// Every row's declared [`MeshEmitter`].
    emitters: Box<[MeshEmitter]>,
}

static MESH_REGISTRY: Slot<MeshRegistry> =
    Slot::new("mesh block rows", &[BLOCK_VIEWS], derive_mesh_registry);

fn derive_mesh_registry(_: &ContentRegistry) -> Result<MeshRegistry, String> {
    // The slot is read through `current`; the loader and workers pin that
    // registry while resolving its block rows.
    Ok(MeshRegistry::from_blocks(Block::all()))
}

impl MeshRegistry {
    /// Bake the tables for `blocks`, indexed by position: `blocks[id]` must
    /// be the block with that id, and `blocks[0]` air.
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

    /// Tables of the current thread's content registry. Mesh jobs pin their
    /// submitter's registry before building.
    pub fn global() -> &'static MeshRegistry {
        MESH_REGISTRY.current()
    }

    /// The cell-scan class of a RAW block id.
    #[inline]
    pub(super) fn cell_class(&self, id: u16) -> u8 {
        class_of(&self.cell, id)
    }

    /// The pad-scan class of a RAW block id.
    #[inline]
    pub(super) fn pad_class(&self, id: u16) -> u8 {
        class_of(&self.pad, id)
    }

    /// The declared emitter of a RAW block id.
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
    // A cell the scan skips outright is never a cube candidate either. Air's
    // row IS the cube family, so without this it would enter the exposure
    // masks' candidate rows and put every open-sky cell back into the visit
    // set the masks exist to shrink.
    if c & SKIP == 0 && fast_cube_candidate(block) {
        c |= FAST_CUBE;
    }
    if block.is_fluid() {
        c |= FLUID;
    }
    c
}

/// Whether a cube-drawn block may take the exposure-mask fast path.
///
/// A block that merges with itself (glass, ice) stays on the per-face path:
/// that same-block cull isn't representable in the opaque-rows exposure masks.
/// Translucent blocks also need the alpha-blended buffer, which the fast path
/// does not emit. Sub-cell shapes never reach here at all — they do not draw
/// through the cube emitter.
fn fast_cube_candidate(block: Block) -> bool {
    !block.is_fluid()
        && !block.merges_with_self()
        && !block.is_translucent()
        && block.mesh_emitter() == MeshEmitter::Cube
}

/// A class-table read at a RAW id, degrading to row 0 (air) past the table.
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
            // A registry that never heard of the id reads it as air.
            assert_eq!(air_only.cell_class(id), cell_class(Block::Air));
            assert_eq!(air_only.pad_class(id), pad_class(Block::Air));
        }
        assert_ne!(all.cell_class(0) & SKIP, 0, "air is skipped");
        assert_eq!(all.cell_class(u16::MAX), all.cell_class(0));
    }
}
