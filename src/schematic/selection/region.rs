use serde::{Deserialize, Serialize};

/// Half-open integer bounds. Selection geometry contains no world data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SelectionBox {
    pub lo: [i32; 3],
    pub hi: [i32; 3],
}

impl SelectionBox {
    pub fn between(a: [i32; 3], b: [i32; 3]) -> Result<Self, String> {
        let lo = std::array::from_fn(|i| a[i].min(b[i]));
        let mut hi = [0; 3];
        for i in 0..3 {
            hi[i] = a[i]
                .max(b[i])
                .checked_add(1)
                .ok_or("Selection coordinate overflow")?;
        }
        let region = Self { lo, hi };
        region.volume().ok_or("Selection volume overflow")?;
        Ok(region)
    }

    pub fn volume(self) -> Option<u64> {
        (0..3).try_fold(1u64, |v, i| {
            let side = i64::from(self.hi[i]) - i64::from(self.lo[i]);
            (side > 0).then_some(side as u64)?.checked_mul(v)
        })
    }

    pub fn contains(self, p: [i32; 3]) -> bool {
        (0..3).all(|i| self.lo[i] <= p[i] && p[i] < self.hi[i])
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let lo = std::array::from_fn(|i| self.lo[i].max(other.lo[i]));
        let hi = std::array::from_fn(|i| self.hi[i].min(other.hi[i]));
        (0..3).all(|i| lo[i] < hi[i]).then_some(Self { lo, hi })
    }

    pub(super) fn subtract(self, other: Self, out: &mut Vec<Self>) {
        let Some(cut) = self.intersection(other) else {
            out.push(self);
            return;
        };
        let mut core = self;
        for axis in 0..3 {
            if core.lo[axis] < cut.lo[axis] {
                let mut part = core;
                part.hi[axis] = cut.lo[axis];
                out.push(part);
                core.lo[axis] = cut.lo[axis];
            }
            if cut.hi[axis] < core.hi[axis] {
                let mut part = core;
                part.lo[axis] = cut.hi[axis];
                out.push(part);
                core.hi[axis] = cut.hi[axis];
            }
        }
    }

    /// Enumerate only when an operation actually needs voxel coordinates.
    pub fn cells(self) -> impl Iterator<Item = [i32; 3]> {
        (self.lo[0]..self.hi[0]).flat_map(move |x| {
            (self.lo[1]..self.hi[1])
                .flat_map(move |y| (self.lo[2]..self.hi[2]).map(move |z| [x, y, z]))
        })
    }
}

pub(super) fn merge(regions: &mut Vec<SelectionBox>) {
    loop {
        let before = regions.len();
        for axis in 0..3 {
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            regions.sort_unstable_by_key(|r| (r.lo[u], r.hi[u], r.lo[v], r.hi[v], r.lo[axis]));
            let mut merged: Vec<SelectionBox> = Vec::with_capacity(regions.len());
            for r in regions.drain(..) {
                if let Some(last) = merged.last_mut() {
                    if last.lo[u] == r.lo[u]
                        && last.hi[u] == r.hi[u]
                        && last.lo[v] == r.lo[v]
                        && last.hi[v] == r.hi[v]
                        && last.hi[axis] == r.lo[axis]
                    {
                        last.hi[axis] = r.hi[axis];
                        continue;
                    }
                }
                merged.push(r);
            }
            *regions = merged;
        }
        if regions.len() == before {
            regions.sort_unstable();
            return;
        }
    }
}
