use crate::block::CellView;
use crate::block::{Block, ShapeFamily};
use crate::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME};

use super::super::face::{Face, FACES};
use super::cube_face::face_index;
use super::pad::{mesh_pad_idx, SectionMeshPad, SECTION_PAD};

const FACE_MASK_WORDS: usize = SECTION_VOLUME / u64::BITS as usize;
pub(super) type ExposedMasks = [[u64; FACE_MASK_WORDS]; FACES.len()];

#[inline]
fn mask_bit(i: usize) -> (usize, u64) {
    (i / u64::BITS as usize, 1u64 << (i % u64::BITS as usize))
}

#[inline]
fn mask_set(masks: &mut ExposedMasks, face: Face, cell: usize) {
    let (word, bit) = mask_bit(cell);
    masks[face_index(face)][word] |= bit;
}

#[inline]
pub(super) fn mask_has(masks: &ExposedMasks, face: Face, cell: usize) -> bool {
    let (word, bit) = mask_bit(cell);
    masks[face_index(face)][word] & bit != 0
}

#[inline]
pub(super) fn pad_cube_fast_candidate(block: Block) -> bool {
    // A block that merges with itself (glass, ice) stays on the per-face path:
    // that same-block cull isn't representable in the opaque-rows exposure
    // masks. Translucent blocks also need the alpha-blended buffer, which the
    // fast path does not emit. Sub-cell shapes never reach here at all — they
    // are not the cube family.
    block != Block::Water
        && !block.merges_with_self()
        && !block.is_translucent()
        && block.shape_family() == ShapeFamily::Cube
        && block != Block::Chest
}

/// `seals_floor(world_pos)`: does that cell's own geometry seal the boundary
/// under it? The SAME seam the per-face path asks (`boxset::cell_seals_face`),
/// passed in rather than re-derived here so the two paths cannot disagree —
/// their agreement is what `mesh::tests::parity` pins.
pub(super) fn build_exposed_masks(
    pad: &SectionMeshPad<'_>,
    origin: (i32, i32, i32),
    seals_floor: &dyn Fn(crate::mathh::IVec3) -> bool,
) -> ExposedMasks {
    const CENTER_BITS: u32 = (1u32 << SECTION_SIZE) - 1;

    #[inline]
    fn row_idx(y: usize, z: usize) -> usize {
        y * SECTION_PAD + z
    }

    #[inline]
    fn set_face_row(masks: &mut ExposedMasks, face: Face, ly: usize, lz: usize, mut bits: u32) {
        while bits != 0 {
            let lx = bits.trailing_zeros() as usize;
            mask_set(masks, face, section_idx(lx, ly, lz));
            bits &= bits - 1;
        }
    }

    let mut masks = [[0u64; FACE_MASK_WORDS]; FACES.len()];
    let mut opaque_rows = [0u32; SECTION_PAD * SECTION_PAD];
    // Cells whose own geometry seals the boundary BENEATH them without being
    // opaque — a lowered cube's floor-flush base, a mod shape with one. Only
    // the PosY cull may read this: a sealed face in the other five directions
    // is plain overdraw, while a sealed top that still draws z-fights the
    // nearly-coplanar cover above it.
    let mut covers_below_rows = [0u32; SECTION_PAD * SECTION_PAD];
    for py in 0..SECTION_PAD {
        for pz in 0..SECTION_PAD {
            let mut row = 0u32;
            let mut covers_row = 0u32;
            for px in 0..SECTION_PAD {
                let block = pad.block_at_pad(px, py, pz);
                if block.is_opaque() || pad.full_slab_stack_at_pad(block, px, py, pz) {
                    row |= 1u32 << px;
                // Air is the overwhelming majority of pad cells; testing it
                // here keeps them off the shape seam entirely.
                } else if block != Block::Air
                    && seals_floor(crate::mathh::IVec3::new(
                        origin.0 - 1 + px as i32,
                        origin.1 - 1 + py as i32,
                        origin.2 - 1 + pz as i32,
                    ))
                {
                    covers_row |= 1u32 << px;
                }
            }
            opaque_rows[row_idx(py, pz)] = row;
            covers_below_rows[row_idx(py, pz)] = covers_row;
        }
    }

    let mut candidate_rows = [0u32; SECTION_SIZE * SECTION_SIZE];
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let mut row = 0u32;
            for lx in 0..SECTION_SIZE {
                let block = pad.block_at_pad(lx + 1, ly + 1, lz + 1);
                if block == Block::Air {
                    continue;
                }
                // Same-material full slab stacks take the cube fast path too; this
                // MUST match the slab-branch fall-through in `section_geometry`.
                let slab_as_cube = block.is_slab()
                    && crate::slab::is_uniform_full_stack(
                        crate::block_state::SlabState::from_cell(
                            pad.cell_states[mesh_pad_idx(lx + 1, ly + 1, lz + 1)],
                        ),
                    );
                if !pad_cube_fast_candidate(block) && !slab_as_cube {
                    continue;
                }
                row |= 1u32 << lx;
            }
            candidate_rows[ly * SECTION_SIZE + lz] = row;
        }
    }

    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let cand = candidate_rows[ly * SECTION_SIZE + lz];
            if cand == 0 {
                continue;
            }
            let (py, pz) = (ly + 1, lz + 1);
            let x_row = opaque_rows[row_idx(py, pz)];
            set_face_row(
                &mut masks,
                Face::PosX,
                ly,
                lz,
                cand & !((x_row >> 2) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::NegX,
                ly,
                lz,
                cand & !(x_row & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::PosY,
                ly,
                lz,
                cand & !(((opaque_rows[row_idx(py + 1, pz)]
                    | covers_below_rows[row_idx(py + 1, pz)])
                    >> 1)
                    & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::NegY,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py - 1, pz)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::PosZ,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py, pz + 1)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::NegZ,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py, pz - 1)] >> 1) & CENTER_BITS),
            );
        }
    }
    masks
}
