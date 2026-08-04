//! The chunk mesher's per-block-id DISPATCH CLASS: one dense byte that answers
//! every "which emitter does this cell use" question the cell scan asks.
//!
//! The scan visits 4096 cells per section and used to re-derive the answer per
//! cell out of five separate registry tables (`from_id`, `shape_family`,
//! `has_box_shape`, `merges_with_self`, `is_translucent`), each behind its own
//! lazy-static check. The classification depends only on the block id, so it is
//! baked once — but it is baked by REPLAYING the scan's own ordered checks, so
//! a class can never disagree with the branch it replaces.

use std::sync::LazyLock;

use petramond_world::block::{Block, ShapeFamily};

/// Air, chest and door cells emit nothing here (chests and doors are dynamic
/// render entities).
pub(super) const SKIP: u8 = 1 << 0;
/// The `Cross` plant family (double-sided diagonal planes).
pub(super) const CROSS: u8 = 1 << 1;
/// The `Crop` plant family (inset, dropped planes).
pub(super) const CROP: u8 = 1 << 2;
pub(super) const TORCH: u8 = 1 << 3;
/// Resolves through the unified box-set emitter (may still fall through to the
/// cube path when the family meshes as a cube or resolves no boxes).
pub(super) const BOXES: u8 = 1 << 4;
/// A bbmodel block.
pub(super) const MODEL: u8 = 1 << 5;
/// Eligible for the exposure-mask cube fast path.
pub(super) const FAST_CUBE: u8 = 1 << 6;
pub(super) const WATER: u8 = 1 << 7;

/// A SECOND dense byte, for the exposure-mask build's pad scan (which asks
/// different questions of every one of 5832 pad cells than the cell scan asks
/// of the section's 4096).
pub(super) const PAD_OPAQUE: u8 = 1 << 0;
pub(super) const PAD_SLAB: u8 = 1 << 1;
/// The only cells whose own geometry can seal the boundary beneath them —
/// non-air box shapes that are not see-through. Everything else (air, water,
/// plants, leaves, plain cubes) is rejected by `boxset::cell_seals_face`
/// anyway, and asking it costs a world read plus a big-table shape-kind load.
pub(super) const PAD_SEALS: u8 = 1 << 2;

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
                if block != Block::Air
                    && block.has_box_shape()
                    && !block.is_transparent()
                    && !block.is_translucent()
                {
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
                let family = block.shape_family();
                let mut c = if block == Block::Air
                    || block == Block::Chest
                    || family == ShapeFamily::Door
                {
                    SKIP
                } else if family == ShapeFamily::Cross {
                    CROSS
                } else if family == ShapeFamily::Crop {
                    CROP
                } else if family == ShapeFamily::Torch {
                    TORCH
                } else if block.has_box_shape() {
                    BOXES
                } else if family == ShapeFamily::Model {
                    MODEL
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
                if block == Block::Water {
                    c |= WATER;
                }
                c
            })
            .collect()
    });
    &CLASSES
}

/// Whether a cube-family block may take the exposure-mask fast path.
///
/// A block that merges with itself (glass, ice) stays on the per-face path:
/// that same-block cull isn't representable in the opaque-rows exposure masks.
/// Translucent blocks also need the alpha-blended buffer, which the fast path
/// does not emit. Sub-cell shapes never reach here at all — they are not the
/// cube family.
fn fast_cube_candidate(block: Block) -> bool {
    block != Block::Water
        && !block.merges_with_self()
        && !block.is_translucent()
        && block.shape_family() == ShapeFamily::Cube
        && block != Block::Chest
}

/// A class-table read at a RAW id. The tables cover the loaded registry, and
/// a raw id can outrun it exactly where `Block::from_id` degrades to air —
/// which is air's class, row 0.
#[inline]
pub(super) fn class_of(table: &[u8], id: u16) -> u8 {
    table.get(id as usize).copied().unwrap_or(table[0])
}
