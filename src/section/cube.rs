//! The section's block cube: 4096 registry ids, stored at the narrowest width
//! that holds them.
//!
//! Block ids are `u16`, but the ids a section actually contains are almost
//! always the engine range — terrain is engine blocks, and even a built
//! structure is usually made of them. Storing every cube as `u16` would double
//! resident world memory (measured: +18.4 MiB of section cubes at render
//! distance 16, +7.7% of the whole world census) to carry a high byte that is
//! zero in every cell.
//!
//! So a cube is NARROW until it holds an id past 255, and widens on the write
//! that first needs it. Both forms are `Arc`-shared, so the off-thread light
//! and mesh pools still take a refcount bump rather than a copy, and the
//! narrow form keeps the process-wide [`uniform_cube`](super::uniform_cube)
//! sharing that ~43% of loaded sections rely on.
//!
//! The workers' padded buffers stay `u16` — one representation on the read
//! side, so the mesher and the light flood never learn this type exists.
//! Filling them from a narrow cube moves LESS memory than a `u16` cube would
//! (4 KiB read + 8 KiB written, against 8 + 8), which is why this is not a
//! memory-for-CPU trade.

use std::sync::Arc;

use crate::chunk::SECTION_VOLUME;

/// Ids at or above this need the wide form.
const NARROW_MAX: u16 = u8::MAX as u16;

/// 4096 block ids, narrow or wide. `Clone` is one refcount bump.
#[derive(Clone)]
pub struct BlockCube {
    repr: Repr,
}

#[derive(Clone)]
enum Repr {
    Narrow(Arc<[u8]>),
    Wide(Arc<[u16]>),
}

impl BlockCube {
    /// A cube every cell of which is `id`, sharing the process-wide buffer for
    /// the narrow ids (all-air sky, all-stone deep, all-water ocean).
    pub(crate) fn uniform(id: u16) -> Self {
        let repr = if id <= NARROW_MAX {
            Repr::Narrow(super::uniform_cube(id as u8))
        } else {
            Repr::Wide(vec![id; SECTION_VOLUME].into())
        };
        Self { repr }
    }

    /// Build from a full id list, taking the narrow form when it fits — the
    /// decode/replica/mod-terrain-fill entry point.
    pub(crate) fn from_ids(ids: &[u16]) -> Self {
        if ids.iter().all(|&id| id <= NARROW_MAX) {
            let narrow: Arc<[u8]> = ids.iter().map(|&id| id as u8).collect();
            Self {
                repr: Repr::Narrow(narrow),
            }
        } else {
            Self {
                repr: Repr::Wide(Arc::from(ids)),
            }
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        match &self.repr {
            Repr::Narrow(b) => b.len(),
            Repr::Wide(b) => b.len(),
        }
    }

    #[inline]
    pub(crate) fn get(&self, i: usize) -> u16 {
        match &self.repr {
            Repr::Narrow(b) => b[i] as u16,
            Repr::Wide(b) => b[i],
        }
    }

    /// Ids in cell order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.len()).map(move |i| self.get(i))
    }

    /// Write one cell, widening the cube if `id` does not fit the narrow form.
    #[inline]
    pub(crate) fn set(&mut self, i: usize, id: u16) {
        match &mut self.repr {
            Repr::Narrow(b) if id <= NARROW_MAX => Arc::make_mut(b)[i] = id as u8,
            Repr::Wide(b) => Arc::make_mut(b)[i] = id,
            Repr::Narrow(_) => {
                self.widen();
                self.set(i, id);
            }
        }
    }

    /// Overwrite every cell with `id`.
    pub(crate) fn fill(&mut self, id: u16) {
        *self = Self::uniform(id);
    }

    /// Copy `n` consecutive ids starting at `src` into `dst` — the bulk path
    /// the mesh pad and the light neighbourhood assemble their rows through.
    /// Widening a narrow row here is a single vectorised pass.
    #[inline]
    pub(crate) fn expand_row_into(&self, src: usize, dst: &mut [u16]) {
        match &self.repr {
            Repr::Narrow(b) => {
                let n = dst.len();
                for (d, &s) in dst.iter_mut().zip(&b[src..src + n]) {
                    *d = s as u16;
                }
            }
            Repr::Wide(b) => dst.copy_from_slice(&b[src..src + dst.len()]),
        }
    }

    /// `(buffer identity, resident bytes)` for the memory census, which counts
    /// each distinct shared buffer once.
    pub(crate) fn heap(&self) -> (usize, u64) {
        match &self.repr {
            Repr::Narrow(b) => (b.as_ptr() as usize, b.len() as u64),
            Repr::Wide(b) => (b.as_ptr() as usize, (b.len() * 2) as u64),
        }
    }

    /// Whether every id in the cube fits one byte — census/diagnostics only.
    #[cfg(test)]
    pub(crate) fn is_narrow(&self) -> bool {
        matches!(self.repr, Repr::Narrow(_))
    }

    fn widen(&mut self) {
        if let Repr::Narrow(b) = &self.repr {
            self.repr = Repr::Wide(b.iter().map(|&v| v as u16).collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the two forms: engine-range content stays one byte
    /// per cell, a single high id widens the cube it lands in, and neither
    /// form loses an id.
    #[test]
    fn a_cube_widens_only_when_an_id_needs_it_and_never_loses_one() {
        let mut c = BlockCube::uniform(0);
        assert!(c.is_narrow());
        c.set(5, 255);
        assert!(c.is_narrow(), "255 still fits a byte");
        assert_eq!(c.get(5), 255);

        c.set(9, 256);
        assert!(!c.is_narrow(), "256 does not");
        assert_eq!((c.get(5), c.get(9), c.get(0)), (255, 256, 0));

        // Widening preserves every cell that was already written.
        let mut c = BlockCube::uniform(7);
        c.set(1, 4095);
        assert_eq!(c.get(0), 7);
        assert_eq!(c.get(1), 4095);
        assert_eq!(c.iter().filter(|&id| id == 7).count(), SECTION_VOLUME - 1);

        // A row expand answers identically from either form.
        let mut narrow = vec![0u16; 16];
        BlockCube::uniform(3).expand_row_into(32, &mut narrow);
        assert_eq!(narrow, vec![3u16; 16]);
        let mut wide = vec![0u16; 16];
        BlockCube::uniform(4000).expand_row_into(32, &mut wide);
        assert_eq!(wide, vec![4000u16; 16]);

        // `from_ids` picks the narrow form exactly when it can.
        assert!(BlockCube::from_ids(&vec![255u16; SECTION_VOLUME]).is_narrow());
        let mut ids = vec![1u16; SECTION_VOLUME];
        ids[SECTION_VOLUME - 1] = 300;
        let wide = BlockCube::from_ids(&ids);
        assert!(!wide.is_narrow());
        assert_eq!(wide.get(SECTION_VOLUME - 1), 300);
    }
}
