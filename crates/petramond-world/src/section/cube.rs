use std::sync::Arc;

use crate::chunk::SECTION_VOLUME;

const NARROW_MAX: u16 = u8::MAX as u16;

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
    pub fn uniform(id: u16) -> Self {
        let repr = if id <= NARROW_MAX {
            Repr::Narrow(super::uniform_cube(id as u8))
        } else {
            Repr::Wide(vec![id; SECTION_VOLUME].into())
        };
        Self { repr }
    }

    pub fn from_ids(ids: &[u16]) -> Self {
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

    #[allow(clippy::len_without_is_empty)]
    #[inline]
    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::Narrow(b) => b.len(),
            Repr::Wide(b) => b.len(),
        }
    }

    #[inline]
    pub fn get(&self, i: usize) -> u16 {
        match &self.repr {
            Repr::Narrow(b) => b[i] as u16,
            Repr::Wide(b) => b[i],
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
    pub fn set(&mut self, i: usize, id: u16) {
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

    #[cfg(any(test, feature = "test-support"))]
    pub fn is_narrow(&self) -> bool {
        matches!(self.repr, Repr::Narrow(_))
    }

    fn widen(&mut self) {
        if let Repr::Narrow(b) = &self.repr {
            self.repr = Repr::Wide(b.iter().map(|&v| v as u16).collect());
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
