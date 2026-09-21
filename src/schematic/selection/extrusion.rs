use super::{region, FaceRect, Selection, SelectionBox, SelectionFace};

pub(super) struct Extrusion {
    before: Vec<SelectionBox>,
    face: SelectionFace,
    offset: i32,
    backing: Vec<SelectionBox>,
    outside: Vec<SelectionBox>,
}

impl Selection {
    pub fn begin_extrusion(&mut self, face: SelectionFace) {
        self.cancel_extrusion();
        let backing = attached_columns(&face, &self.regions);
        let mut outside = self.regions.clone();
        for column in &backing {
            let mut remaining = Vec::new();
            for r in outside {
                r.subtract(*column, &mut remaining);
            }
            outside = remaining;
        }
        region::merge(&mut outside);
        self.extrusion = Some(Extrusion {
            before: self.regions.clone(),
            face,
            offset: 0,
            backing,
            outside,
        });
    }

    /// Signed outward displacement. Every preview derives from the original geometry.
    pub fn extrude(&mut self, offset: i32) -> Result<(), String> {
        let Some(edit) = &mut self.extrusion else {
            return Ok(());
        };
        if offset == edit.offset {
            return Ok(());
        }
        let plane = i64::from(edit.face.plane) + i64::from(edit.face.normal) * i64::from(offset);
        let plane = i32::try_from(plane).map_err(|_| "Selection coordinate overflow")?;
        let mut next;
        if offset < 0 {
            next = edit.outside.clone();
            for mut column in edit.backing.iter().copied() {
                if edit.face.normal > 0 {
                    column.hi[edit.face.axis] = column.hi[edit.face.axis].min(plane);
                } else {
                    column.lo[edit.face.axis] = column.lo[edit.face.axis].max(plane);
                }
                if column.lo[edit.face.axis] < column.hi[edit.face.axis] {
                    next.push(column);
                }
            }
        } else {
            next = edit.before.clone();
        }
        if offset > 0 {
            for sweep in edit.face.sweep(plane) {
                let mut added = vec![sweep];
                for r in &next {
                    let mut remaining = Vec::new();
                    for a in added {
                        a.subtract(*r, &mut remaining);
                    }
                    added = remaining;
                    if added.is_empty() {
                        break;
                    }
                }
                next.extend(added);
            }
        }
        if offset != 0 {
            region::merge(&mut next);
        }
        let count = next
            .iter()
            .try_fold(0u64, |count, r| count.checked_add(r.volume()?))
            .ok_or("Selection volume overflow")?;
        edit.offset = offset;
        if self.regions != next {
            self.regions = next;
            self.count = count;
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(())
    }

    pub fn extrusion_face(&self) -> Option<SelectionFace> {
        let edit = self.extrusion.as_ref()?;
        let mut face = edit.face.clone();
        face.plane =
            (i64::from(face.plane) + i64::from(face.normal) * i64::from(edit.offset)) as i32;
        Some(face)
    }

    pub fn finish_extrusion(&mut self) {
        let Some(edit) = self.extrusion.take() else {
            return;
        };
        if self.regions != edit.before {
            let next = std::mem::replace(&mut self.regions, edit.before);
            self.commit(next, self.count);
        }
    }

    pub fn cancel_extrusion(&mut self) -> bool {
        let Some(edit) = self.extrusion.take() else {
            return false;
        };
        if self.regions != edit.before {
            self.regions = edit.before;
            self.restored();
        }
        true
    }
}

// Follow the selected volume immediately behind the face. A contraction must
// stop at gaps instead of erasing a different island further along the axis.
fn attached_columns(face: &SelectionFace, regions: &[SelectionBox]) -> Vec<SelectionBox> {
    let axis = face.axis;
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let mut frontier: Vec<_> = face.rectangles.iter().map(|r| (*r, face.plane)).collect();
    let mut result = Vec::new();
    while let Some((rect, plane)) = frontier.pop() {
        for r in regions {
            let behind = if face.normal > 0 {
                r.lo[axis] < plane && plane <= r.hi[axis]
            } else {
                r.lo[axis] <= plane && plane < r.hi[axis]
            };
            if !behind {
                continue;
            }
            let lo = [rect.lo[0].max(r.lo[u]), rect.lo[1].max(r.lo[v])];
            let hi = [rect.hi[0].min(r.hi[u]), rect.hi[1].min(r.hi[v])];
            if (0..2).any(|i| lo[i] >= hi[i]) {
                continue;
            }
            let mut column = *r;
            column.lo[u] = lo[0];
            column.lo[v] = lo[1];
            column.hi[u] = hi[0];
            column.hi[v] = hi[1];
            let end = if face.normal > 0 {
                column.hi[axis] = plane;
                column.lo[axis]
            } else {
                column.lo[axis] = plane;
                column.hi[axis]
            };
            result.push(column);
            frontier.push((FaceRect { lo, hi }, end));
        }
    }
    result
}

#[cfg(test)]
mod tests;
