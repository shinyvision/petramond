use crate::GenRng;

/// Draw helpers over [`GenRng`] for builders that roll many small choices from one stream.
pub trait Draw {
    fn unit(&mut self) -> f32;

    #[inline]
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Inclusive on both ends.
    #[inline]
    fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        (lo + (self.unit() * (hi - lo + 1) as f32) as i32).min(hi)
    }

    #[inline]
    fn roll(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    #[inline]
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.int(0, items.len() as i32 - 1) as usize]
    }

    /// One entry chosen in proportion to its weight.
    #[inline]
    fn weighted<'a, T>(&mut self, entries: &'a [(T, f32)]) -> &'a T {
        let total: f32 = entries.iter().map(|(_, w)| w).sum();
        let mut r = self.unit() * total;
        for (value, weight) in entries {
            r -= weight;
            if r < 0.0 {
                return value;
            }
        }
        &entries[entries.len() - 1].0
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.int(0, i as i32) as usize;
            items.swap(i, j);
        }
    }
}

impl Draw for GenRng {
    #[inline]
    fn unit(&mut self) -> f32 {
        self.next_f32()
    }
}
