mod extrusion;
mod region;
mod surface;
pub use region::SelectionBox;
pub use surface::{FaceRect, SelectionFace, SelectionSurface};

/// Disjoint boxes describing a union, including holes and disconnected islands.
#[derive(Default)]
pub struct Selection {
    regions: Vec<SelectionBox>,
    count: u64,
    revision: u64,
    undo: Vec<Vec<SelectionBox>>,
    redo: Vec<Vec<SelectionBox>>,
    extrusion: Option<extrusion::Extrusion>,
}

impl Selection {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn regions(&self) -> &[SelectionBox] {
        &self.regions
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn cells(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.regions.iter().copied().flat_map(SelectionBox::cells)
    }
    pub fn contains(&self, p: [i32; 3]) -> bool {
        self.regions.iter().any(|r| r.contains(p))
    }
    pub fn len(&self) -> u64 {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
    pub fn clear(&mut self) {
        self.cancel_extrusion();
        if !self.is_empty() {
            self.commit(Vec::new(), 0);
        }
    }

    pub fn region(&mut self, a: [i32; 3], b: [i32; 3], remove: bool) -> Result<(), String> {
        self.cancel_extrusion();
        let region = SelectionBox::between(a, b)?;
        let mut next = Vec::new();
        if remove {
            if !self
                .regions
                .iter()
                .any(|r| r.intersection(region).is_some())
            {
                return Ok(());
            }
            for r in &self.regions {
                r.subtract(region, &mut next);
            }
        } else {
            let mut added = vec![region];
            for r in &self.regions {
                let mut remaining = Vec::new();
                for a in added {
                    a.subtract(*r, &mut remaining);
                }
                added = remaining;
                if added.is_empty() {
                    return Ok(());
                }
            }
            next.clone_from(&self.regions);
            next.extend(added);
        }
        region::merge(&mut next);
        let count = next
            .iter()
            .try_fold(0u64, |count, r| count.checked_add(r.volume()?))
            .ok_or("Selection volume overflow")?;
        self.commit(next, count);
        Ok(())
    }

    fn commit(&mut self, next: Vec<SelectionBox>, count: u64) {
        self.redo.clear();
        self.undo.push(std::mem::replace(&mut self.regions, next));
        self.count = count;
        self.revision = self.revision.wrapping_add(1);
        while self.undo.len() > 1
            && (self.undo.len() > 64
                || self
                    .undo
                    .iter()
                    .map(|r| r.len() * std::mem::size_of::<SelectionBox>())
                    .sum::<usize>()
                    > 4 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.cancel_extrusion() {
            return true;
        }
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.regions, previous));
        self.restored();
        true
    }
    pub fn redo(&mut self) -> bool {
        self.cancel_extrusion();
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.regions, next));
        self.restored();
        true
    }
    fn restored(&mut self) {
        self.count = self
            .regions
            .iter()
            .map(|r| r.volume().expect("validated selection"))
            .sum();
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn raycast(
        &self,
        eye: petramond_math::world_pos::WorldPos,
        dir: petramond_math::math::Vec3,
        max: f32,
    ) -> Option<[i32; 3]> {
        if !dir.is_finite() || !max.is_finite() || max < 0.0 || dir.length_squared() < 1e-8 {
            return None;
        }
        let mut p = [
            eye.x.floor() as i32,
            eye.y.floor() as i32,
            eye.z.floor() as i32,
        ];
        let origin = [eye.x, eye.y, eye.z];
        let step: [i32; 3] = std::array::from_fn(|i| if dir[i] < 0.0 { -1 } else { 1 });
        let delta: [f64; 3] = std::array::from_fn(|i| 1.0 / f64::from(dir[i]).abs());
        let mut next: [f64; 3] = std::array::from_fn(|i| {
            if dir[i] == 0.0 {
                f64::INFINITY
            } else {
                (f64::from(p[i] + i32::from(step[i] > 0)) - origin[i]) / f64::from(dir[i])
            }
        });
        loop {
            if self.contains(p) {
                return Some(p);
            }
            let axis = (0..3).min_by(|a, b| next[*a].total_cmp(&next[*b])).unwrap();
            if next[axis] > f64::from(max) {
                return None;
            }
            p[axis] += step[axis];
            next[axis] += delta[axis];
        }
    }
}

#[cfg(test)]
mod tests;
