use petramond_world::block::CellView;
use petramond_world::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME};

use super::super::face::{Face, FACES};
use super::cell_class::{FAST_CUBE, PAD_OPAQUE, PAD_OPAQUE_FLUID, PAD_SEALS, PAD_SLAB, SKIP};
use super::cube_face::face_index;
use super::neighbourhood::Neighbourhood;
use super::pad::{mesh_pad_idx, SECTION_PAD};

const FACE_MASK_WORDS: usize = SECTION_VOLUME / u64::BITS as usize;
const _: () = assert!((u64::BITS as usize).is_multiple_of(SECTION_SIZE));

pub(super) struct ExposedMasks {
    faces: [[u64; FACE_MASK_WORDS]; FACES.len()],
    /// Per `(ly, lz)` row, the X positions the cell scan must actually visit:
    /// every non-air cell that is not a cube fast-path candidate, plus the
    /// candidates that draw at least one face. A buried solid row is ZERO, so
    /// the scan skips all sixteen cells without touching them — which is most
    /// of every underground section.
    visit: [u16; SECTION_SIZE * SECTION_SIZE],
}

pub(super) const VISIT_ALL: [u16; SECTION_SIZE * SECTION_SIZE] =
    [u16::MAX; SECTION_SIZE * SECTION_SIZE];

impl ExposedMasks {
    #[inline]
    pub(super) fn visit_rows(&self) -> &[u16; SECTION_SIZE * SECTION_SIZE] {
        &self.visit
    }
}

#[inline]
fn mask_bit(i: usize) -> (usize, u64) {
    (i / u64::BITS as usize, 1u64 << (i % u64::BITS as usize))
}

#[inline]
pub(super) fn mask_has(masks: &ExposedMasks, face: Face, cell: usize) -> bool {
    let (word, bit) = mask_bit(cell);
    masks.faces[face_index(face)][word] & bit != 0
}

pub(super) fn build_exposed_masks(nb: &Neighbourhood<'_>) -> ExposedMasks {
    let pad = nb.pad();
    let origin = nb.origin();
    const CENTER_BITS: u32 = (1u32 << SECTION_SIZE) - 1;

    #[inline]
    fn row_idx(y: usize, z: usize) -> usize {
        y * SECTION_PAD + z
    }

    #[inline]
    fn set_face_row(
        masks: &mut ExposedMasks,
        exposed: &mut u32,
        face: Face,
        ly: usize,
        lz: usize,
        bits: u32,
    ) {
        *exposed |= bits;
        let (word, bit) = mask_bit(section_idx(0, ly, lz));
        masks.faces[face_index(face)][word] |= u64::from(bits) * bit;
    }

    let registry = nb.registry();
    let mut masks = ExposedMasks {
        faces: [[0u64; FACE_MASK_WORDS]; FACES.len()],
        visit: [0u16; SECTION_SIZE * SECTION_SIZE],
    };
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
                let i = mesh_pad_idx(px, py, pz);
                let c = registry.pad_class(pad.blocks[i]);
                if c & PAD_OPAQUE != 0
                    || (c & PAD_SLAB != 0
                        && petramond_world::block_state::SlabState::from_cell(pad.cell_states[i])
                            .is_full())
                    || (c & PAD_OPAQUE_FLUID != 0
                        && pad.fluid_fills_local(
                            px as i32 - 1,
                            py as i32 - 1,
                            pz as i32 - 1,
                            pad.table.block(pad.blocks[i]),
                        ))
                {
                    row |= 1u32 << px;
                } else if c & PAD_SEALS != 0
                    && nb.seals_floor(
                        origin - glam::IVec3::ONE
                            + glam::IVec3::new(px as i32, py as i32, pz as i32),
                    )
                {
                    covers_row |= 1u32 << px;
                }
            }
            opaque_rows[row_idx(py, pz)] = row;
            covers_below_rows[row_idx(py, pz)] = covers_row;
        }
    }

    let mut candidate_rows = [0u32; SECTION_SIZE * SECTION_SIZE];
    let mut work_rows = [0u32; SECTION_SIZE * SECTION_SIZE];
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let mut row = 0u32;
            let mut work = 0u32;
            for lx in 0..SECTION_SIZE {
                let i = mesh_pad_idx(lx + 1, ly + 1, lz + 1);
                let id = pad.blocks[i];
                if registry.cell_class(id) & SKIP == 0 {
                    work |= 1u32 << lx;
                }
                if registry.cell_class(id) & FAST_CUBE == 0
                    && (registry.pad_class(id) & PAD_SLAB == 0
                        || !petramond_world::slab::is_uniform_full_stack(
                            petramond_world::block_state::SlabState::from_cell(pad.cell_states[i]),
                        ))
                {
                    continue;
                }
                row |= 1u32 << lx;
            }
            candidate_rows[ly * SECTION_SIZE + lz] = row;
            work_rows[ly * SECTION_SIZE + lz] = work;
        }
    }

    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let cand = candidate_rows[ly * SECTION_SIZE + lz];
            if cand == 0 {
                masks.visit[ly * SECTION_SIZE + lz] = work_rows[ly * SECTION_SIZE + lz] as u16;
                continue;
            }
            let (py, pz) = (ly + 1, lz + 1);
            let mut exposed = 0u32;
            let x_row = opaque_rows[row_idx(py, pz)];
            set_face_row(
                &mut masks,
                &mut exposed,
                Face::PosX,
                ly,
                lz,
                cand & !((x_row >> 2) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                &mut exposed,
                Face::NegX,
                ly,
                lz,
                cand & !(x_row & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                &mut exposed,
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
                &mut exposed,
                Face::NegY,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py - 1, pz)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                &mut exposed,
                Face::PosZ,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py, pz + 1)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                &mut exposed,
                Face::NegZ,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py, pz - 1)] >> 1) & CENTER_BITS),
            );
            let work = work_rows[ly * SECTION_SIZE + lz];
            masks.visit[ly * SECTION_SIZE + lz] = ((work & !cand) | (cand & exposed)) as u16;
        }
    }
    masks
}
