use std::sync::Arc;

use crate::chunk::SECTION_VOLUME;

const NARROW_MAX: u16 = u8::MAX as u16;

#[derive(Clone)]
pub struct BlockCube {
    repr: Repr,
    stamp: u64,
}

/// A value no other cube state has carried: every construction and every write takes one, and
/// a clone keeps its source's (the content is the same). Per-thread ranges, so writers on
/// different threads never contend on it.
fn fresh_stamp() -> u64 {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};
    static THREADS: AtomicU64 = AtomicU64::new(1);
    thread_local! {
        static NEXT: Cell<u64> = const { Cell::new(0) };
    }
    NEXT.with(|next| {
        let mut stamp = next.get();
        if stamp == 0 {
            stamp = THREADS.fetch_add(1, Ordering::Relaxed) << 40;
        }
        next.set(stamp + 1);
        stamp
    })
}

#[derive(Clone)]
enum Repr {
    Narrow(Arc<[u8]>),
    Wide(Arc<[u16]>),
}

impl BlockCube {
    pub fn uniform(id: u16) -> Self {
        let repr = if id <= NARROW_MAX {
            Repr::Narrow(super::uniform_cube(id as u8))
        } else {
            Repr::Wide(vec![id; SECTION_VOLUME].into())
        };
        Self {
            repr,
            stamp: fresh_stamp(),
        }
    }

    pub fn from_ids(ids: &[u16]) -> Self {
        if ids.iter().all(|&id| id <= NARROW_MAX) {
            let narrow: Arc<[u8]> = ids.iter().map(|&id| id as u8).collect();
            Self {
                repr: Repr::Narrow(narrow),
                stamp: fresh_stamp(),
            }
        } else {
            Self {
                repr: Repr::Wide(Arc::from(ids)),
                stamp: fresh_stamp(),
            }
        }
    }

    #[allow(clippy::len_without_is_empty)]
    #[inline]
    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::Narrow(b) => b.len(),
            Repr::Wide(b) => b.len(),
        }
    }

    /// The ids as bytes when every id in the cube fits one.
    #[inline]
    pub fn as_narrow(&self) -> Option<&[u8]> {
        match &self.repr {
            Repr::Narrow(b) => Some(b),
            Repr::Wide(_) => None,
        }
    }

    #[inline]
    pub fn get(&self, i: usize) -> u16 {
        match &self.repr {
            Repr::Narrow(b) => b[i] as u16,
            Repr::Wide(b) => b[i],
        }
    }

    /// Appends every id as little-endian bytes, in cell order.
    pub fn write_le_bytes(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.resize(start + 2 * self.len(), 0);
        let dst = out[start..].as_chunks_mut::<2>().0;
        match &self.repr {
            Repr::Narrow(b) => {
                for (d, &id) in dst.iter_mut().zip(b.iter()) {
                    d[0] = id;
                }
            }
            Repr::Wide(b) => {
                for (d, &id) in dst.iter_mut().zip(b.iter()) {
                    *d = id.to_le_bytes();
                }
            }
        }
    }

    pub fn copy_ids(&self, out: &mut [u16]) {
        match &self.repr {
            Repr::Narrow(b) => {
                for (o, &id) in out.iter_mut().zip(b.iter()) {
                    *o = u16::from(id);
                }
            }
            Repr::Wide(b) => out.copy_from_slice(b),
        }
    }

    #[inline]
    pub fn cells_where(&self, mut pred: impl FnMut(u16) -> bool, mut f: impl FnMut(usize)) {
        match &self.repr {
            Repr::Narrow(b) => b.iter().enumerate().for_each(|(i, &id)| {
                if pred(u16::from(id)) {
                    f(i)
                }
            }),
            Repr::Wide(b) => b.iter().enumerate().for_each(|(i, &id)| {
                if pred(id) {
                    f(i)
                }
            }),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.len()).map(move |i| self.get(i))
    }

    #[inline]
    pub fn for_each_id(&self, mut f: impl FnMut(u16)) {
        match &self.repr {
            Repr::Narrow(b) => b.iter().for_each(|&id| f(u16::from(id))),
            Repr::Wide(b) => b.iter().for_each(|&id| f(id)),
        }
    }

    /// Whether the `len` cells from `start` all hold `id`.
    #[inline]
    pub fn run_is(&self, start: usize, len: usize, id: u16) -> bool {
        match &self.repr {
            Repr::Narrow(b) => {
                id <= NARROW_MAX && b[start..start + len].iter().all(|&v| v == id as u8)
            }
            Repr::Wide(b) => b[start..start + len].iter().all(|&v| v == id),
        }
    }

    /// Changes whenever the ids may have: equal stamps mean equal content.
    #[inline]
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    #[inline]
    pub fn set(&mut self, i: usize, id: u16) {
        self.stamp = fresh_stamp();
        match &mut self.repr {
            Repr::Narrow(b) if id <= NARROW_MAX => Arc::make_mut(b)[i] = id as u8,
            Repr::Wide(b) => Arc::make_mut(b)[i] = id,
            Repr::Narrow(_) => {
                self.widen();
                self.set(i, id);
            }
        }
    }

    pub fn fill(&mut self, id: u16) {
        *self = Self::uniform(id);
    }

    #[inline]
    pub fn expand_row_into(&self, src: usize, dst: &mut [u16]) {
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

    pub fn heap(&self) -> (usize, u64) {
        match &self.repr {
            Repr::Narrow(b) => (b.as_ptr() as usize, b.len() as u64),
            Repr::Wide(b) => (b.as_ptr() as usize, (b.len() * 2) as u64),
        }
    }

    #[inline]
    pub fn is_narrow(&self) -> bool {
        matches!(self.repr, Repr::Narrow(_))
    }

    /// `len` cells from `src` as bytes, when every id of the cube fits one.
    #[inline]
    pub fn narrow_row(&self, src: usize, len: usize) -> Option<&[u8]> {
        match &self.repr {
            Repr::Narrow(b) => Some(&b[src..src + len]),
            Repr::Wide(_) => None,
        }
    }

    /// The cells, writable, for ids up to `max_id`: a narrow cube that can't hold it widens.
    pub fn cells_mut(&mut self, max_id: u16) -> CubeCells<'_> {
        self.stamp = fresh_stamp();
        if max_id > NARROW_MAX {
            self.widen();
        }
        match &mut self.repr {
            Repr::Narrow(b) => CubeCells::Narrow(Arc::make_mut(b)),
            Repr::Wide(b) => CubeCells::Wide(Arc::make_mut(b)),
        }
    }

    fn widen(&mut self) {
        if let Repr::Narrow(b) = &self.repr {
            self.repr = Repr::Wide(b.iter().map(|&v| v as u16).collect());
        }
    }
}

/// A [`BlockCube`]'s cells borrowed for a batch of writes.
pub enum CubeCells<'a> {
    Narrow(&'a mut [u8]),
    Wide(&'a mut [u16]),
}

impl CubeCells<'_> {
    #[inline]
    pub fn get(&self, i: usize) -> u16 {
        match self {
            CubeCells::Narrow(b) => b[i] as u16,
            CubeCells::Wide(b) => b[i],
        }
    }

    /// Writes `id`, which must fit the width the cells were borrowed for.
    #[inline]
    pub fn set(&mut self, i: usize, id: u16) {
        match self {
            CubeCells::Narrow(b) => b[i] = id as u8,
            CubeCells::Wide(b) => b[i] = id,
        }
    }

    /// Whether these cells can hold `id` without widening.
    #[inline]
    pub fn holds(&self, id: u16) -> bool {
        matches!(self, CubeCells::Wide(_)) || id <= NARROW_MAX
    }

    #[inline]
    pub fn run_is(&self, start: usize, len: usize, id: u16) -> bool {
        match self {
            CubeCells::Narrow(b) => {
                id <= NARROW_MAX && b[start..start + len].iter().all(|&v| v == id as u8)
            }
            CubeCells::Wide(b) => b[start..start + len].iter().all(|&v| v == id),
        }
    }
}

impl PartialEq for BlockCube {
    fn eq(&self, other: &Self) -> bool {
        match (&self.repr, &other.repr) {
            (Repr::Narrow(a), Repr::Narrow(b)) => Arc::ptr_eq(a, b) || a == b,
            (Repr::Wide(a), Repr::Wide(b)) => Arc::ptr_eq(a, b) || a == b,
            _ => self.iter().eq(other.iter()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let mut c = BlockCube::uniform(7);
        c.set(1, 4095);
        assert_eq!(c.get(0), 7);
        assert_eq!(c.get(1), 4095);
        assert_eq!(c.iter().filter(|&id| id == 7).count(), SECTION_VOLUME - 1);

        let mut narrow = vec![0u16; 16];
        BlockCube::uniform(3).expand_row_into(32, &mut narrow);
        assert_eq!(narrow, vec![3u16; 16]);
        let mut wide = vec![0u16; 16];
        BlockCube::uniform(4000).expand_row_into(32, &mut wide);
        assert_eq!(wide, vec![4000u16; 16]);

        assert!(BlockCube::from_ids(&vec![255u16; SECTION_VOLUME]).is_narrow());
        let mut ids = vec![1u16; SECTION_VOLUME];
        ids[SECTION_VOLUME - 1] = 300;
        let wide = BlockCube::from_ids(&ids);
        assert!(!wide.is_narrow());
        assert_eq!(wide.get(SECTION_VOLUME - 1), 300);
    }
}
