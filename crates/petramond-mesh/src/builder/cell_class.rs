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

use std::sync::LazyLock;

use petramond_world::block::{Block, MeshEmitter};

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

#[inline]
pub(super) fn pad_classes() -> &'static [u8] {
    static CLASSES: LazyLock<Box<[u8]>> = LazyLock::new(|| {
        Block::all()
            .iter()
            .map(|&block| {
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
                let seals_by_shape = block.has_box_shape()
                    && !block.is_transparent()
                    && !block.is_translucent();
                if block != Block::Air && (seals_by_shape || block.is_snow_bedded()) {
                    c |= PAD_SEALS;
                }
                c
            })
            .collect()
    });
    &CLASSES
}

/// The whole class table. The cell scan and the exposure-mask build both take
/// it ONCE and index it per cell, so 4096 cells cost 4096 byte loads rather
/// than 4096 lazy-static checks.
#[inline]
pub(super) fn cell_classes() -> &'static [u8] {
    static CLASSES: LazyLock<Box<[u8]>> = LazyLock::new(|| {
        Block::all()
            .iter()
            .map(|&block| {
                let mut c = if block == Block::Air
                    || block.flags().invisible()
                    || block.mesh_emitter() == MeshEmitter::Nothing
                {
                    SKIP
                } else {
                    0
                };
                // A cell the scan skips outright is never a cube candidate either.
                // Air's row IS the cube family, so without this it would enter the
                // exposure masks' candidate rows and put every open-sky cell back
                // into the visit set the masks exist to shrink.
                if c & SKIP == 0 && fast_cube_candidate(block) {
                    c |= FAST_CUBE;
                }
                if block.is_fluid() {
                    c |= FLUID;
                }
                c
            })
            .collect()
    });
    &CLASSES
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

/// A class-table read at a RAW id. The tables cover the loaded registry, and
/// a raw id can outrun it exactly where `Block::from_id` degrades to air —
/// which is air's class, row 0.
#[inline]
pub(super) fn class_of(table: &[u8], id: u16) -> u8 {
    table.get(id as usize).copied().unwrap_or(table[0])
}

/// Every row's declared [`MeshEmitter`], by block id — what the scan
/// dispatches a non-skipped, non-fluid cell on.
#[inline]
pub(super) fn emitters() -> &'static [MeshEmitter] {
    static EMITTERS: LazyLock<Box<[MeshEmitter]>> =
        LazyLock::new(|| Block::all().iter().map(|b| b.mesh_emitter()).collect());
    &EMITTERS
}

/// An emitter-table read at a RAW id, degrading like [`class_of`].
#[inline]
pub(super) fn emitter_of(table: &[MeshEmitter], id: u16) -> MeshEmitter {
    table.get(id as usize).copied().unwrap_or(table[0])
}
