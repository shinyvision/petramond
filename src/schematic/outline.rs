use super::Selection;
use std::collections::BTreeMap;

impl Selection {
    /// Creases of the union, computed from box faces without visiting its interior.
    pub fn outline(&self) -> Vec<[[i32; 3]; 2]> {
        let mut lines = BTreeMap::<(usize, i32, i32), BTreeMap<i32, u8>>::new();
        for face in self.surface().faces() {
            let axis = face.axis;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let side = usize::from(face.normal > 0);
            for corners in face.quads() {
                for i in 0..4 {
                    let a = corners[i];
                    let b = corners[(i + 1) % 4];
                    let along = if a[u] != b[u] { u } else { v };
                    let events = lines
                        .entry((along, a[(along + 1) % 3], a[(along + 2) % 3]))
                        .or_default();
                    *events.entry(a[along]).or_default() ^= 1 << (axis * 2 + side);
                    *events.entry(b[along]).or_default() ^= 1 << (axis * 2 + side);
                }
            }
        }
        let mut result = Vec::new();
        for ((axis, u, v), events) in lines {
            let mut mask = 0;
            let mut start = 0;
            for (position, change) in events {
                if change == 0 {
                    continue;
                }
                if mask != 0 {
                    let mut a = [0; 3];
                    a[axis] = start;
                    a[(axis + 1) % 3] = u;
                    a[(axis + 2) % 3] = v;
                    let mut b = a;
                    b[axis] = position;
                    result.push([a, b]);
                }
                mask ^= change;
                start = position;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests;
