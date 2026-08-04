use petramond_world::block::CellView;
use petramond_world::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME};

use super::super::face::{Face, FACES};
use super::cell_class::{class_of, FAST_CUBE, PAD_OPAQUE, PAD_SEALS, PAD_SLAB, SKIP};
use super::cube_face::face_index;
use super::pad::{mesh_pad_idx, SectionMeshPad, SECTION_PAD};

const FACE_MASK_WORDS: usize = SECTION_VOLUME / u64::BITS as usize;

/// Per-face exposure bitsets plus the derived WORK bitset the cell scan
/// iterates.
pub(super) struct ExposedMasks {
    faces: [[u64; FACE_MASK_WORDS]; FACES.len()],
    /// Per `(ly, lz)` row, the X positions the cell scan must actually visit:
    /// every non-air cell that is not a cube fast-path candidate, plus the
    /// candidates that draw at least one face. A buried solid row is ZERO, so
    /// the scan skips all sixteen cells without touching them — which is most
    /// of every underground section.
    visit: [u16; SECTION_SIZE * SECTION_SIZE],
}

/// The scan's fallback when no exposure masks were built (the far-leaf LOD
/// pass, which has no pad): visit every cell.
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
fn mask_set(masks: &mut ExposedMasks, face: Face, cell: usize) {
    let (word, bit) = mask_bit(cell);
    masks.faces[face_index(face)][word] |= bit;
}

#[inline]
pub(super) fn mask_has(masks: &ExposedMasks, face: Face, cell: usize) -> bool {
    let (word, bit) = mask_bit(cell);
    masks.faces[face_index(face)][word] & bit != 0
}

/// `seals_floor(world_pos)`: does that cell's own geometry seal the boundary
/// under it? The SAME seam the per-face path asks (`boxset::cell_seals_face`),
/// passed in rather than re-derived here so the two paths cannot disagree —
/// their agreement is what `mesh::tests::parity` pins.
pub(super) fn build_exposed_masks(
    pad: &SectionMeshPad<'_>,
    origin: (i32, i32, i32),
    seals_floor: &dyn Fn(petramond_world::mathh::IVec3) -> bool,
) -> ExposedMasks {
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
        mut bits: u32,
    ) {
        *exposed |= bits;
        while bits != 0 {
            let lx = bits.trailing_zeros() as usize;
            mask_set(masks, face, section_idx(lx, ly, lz));
            bits &= bits - 1;
        }
    }

    let pad_class = super::cell_class::pad_classes();
    let classes = super::cell_class::cell_classes();
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
                let c = pad_class[pad.blocks[i] as usize];
                if c & PAD_OPAQUE != 0
                    || (c & PAD_SLAB != 0
                        && petramond_world::block_state::SlabState::from_cell(pad.cell_states[i]).is_full())
                {
                    row |= 1u32 << px;
                // Air, water, plants and plain cubes are the overwhelming
                // majority of pad cells; the dense flag keeps every one of
                // them off the shape seam.
                } else if c & PAD_SEALS != 0
                    && seals_floor(petramond_world::mathh::IVec3::new(
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
    // Cells the scan has real work for whatever their exposure: plants,
    // torches, box shapes, models, water, glass — everything that is neither
    // air/chest/door nor a plain cube.
    let mut work_rows = [0u32; SECTION_SIZE * SECTION_SIZE];
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let mut row = 0u32;
            let mut work = 0u32;
            for lx in 0..SECTION_SIZE {
                let i = mesh_pad_idx(lx + 1, ly + 1, lz + 1);
                let id = pad.blocks[i];
                if class_of(classes, id) & SKIP == 0 {
                    work |= 1u32 << lx;
                }
                if class_of(classes, id) & FAST_CUBE == 0 {
                    // Same-material full slab stacks take the cube fast path too;
                    // this MUST match the slab-branch fall-through in
                    // `section_geometry`.
                    if class_of(pad_class, id) & PAD_SLAB == 0
                        || !petramond_world::slab::is_uniform_full_stack(
                            petramond_world::block_state::SlabState::from_cell(pad.cell_states[i]),
                        )
                    {
                        continue;
                    }
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
                // No cube candidate here; only the non-cube work stands.
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
