//! The terrain fill's own surface, for the derivations whose answer depends
//! on what the fill leaves above it: open sky and the sea are not cave rock,
//! but the cave model alone reads them as rock.

use std::sync::Arc;

use super::*;

impl CaveField {
    /// The column's highest solid density cell — the surface the section
    /// fill lays terrain under. A pure function of the seed, read per chunk.
    pub(crate) fn density_surface(&self, x: i32, z: i32) -> i32 {
        let sec = SECTION_SIZE as i32;
        let heights = self.density_surface_tile([x.div_euclid(sec), z.div_euclid(sec)]);
        heights[(z.rem_euclid(sec) * sec + x.rem_euclid(sec)) as usize]
    }

    /// The density surfaces of one 16×16 chunk, `z*16 + x`.
    pub(crate) fn density_surface_tile(&self, chunk: [i32; 2]) -> Arc<[i32]> {
        let sec = SECTION_SIZE as i32;
        self.memos()
            .density_surfaces
            .get_or_insert((self.seed, chunk), || {
                crate::density::surface::surface_heights(
                    &self.terrain,
                    chunk[0] * sec,
                    chunk[1] * sec,
                    SECTION_SIZE,
                    SECTION_SIZE,
                )
                .into()
            })
    }
}
