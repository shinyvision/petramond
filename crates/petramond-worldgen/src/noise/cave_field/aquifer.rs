use super::*;
use crate::data::underground::Aquifer;

impl CaveField {
    pub(super) fn aquifer_at(&self, lat: &CaveLattice, [x, y, z]: [i32; 3]) -> Option<Aquifer> {
        let (lo, hi) = self.underground.aquifer_y_span?;
        if y < lo || y > hi || lat.no_aquifer {
            return None;
        }
        let aquifer = self.underground.aquifer(self.biome_id_lat(lat, x, y, z))?;
        (y <= aquifer.level).then_some(aquifer)
    }

    /// [`Self::aquifer_at`] for the cursor's own column.
    pub(super) fn aquifer_at_col(&self, c: &mut Col, y: i32) -> Option<Aquifer> {
        let (lo, hi) = self.underground.aquifer_y_span?;
        if y < lo || y > hi || c.lat.no_aquifer {
            return None;
        }
        let aquifer = self.underground.aquifer(self.biome_id_col(c, y))?;
        (y <= aquifer.level).then_some(aquifer)
    }

    pub(super) fn cut_from_col(&self, c: &mut Col, y: i32, gate: bool, interior: bool) -> CaveCut {
        let treatment = c.lat.volumes.at([c.x, y, c.z]);
        self.cut_from_col_treated(c, y, gate, interior, treatment)
    }

    /// [`Self::cut_from_col`] with the cell's positioned-field treatment
    /// already looked up.
    pub(super) fn cut_from_col_treated(
        &self,
        c: &mut Col,
        y: i32,
        gate: bool,
        interior: bool,
        treatment: super::volumes::Cell,
    ) -> CaveCut {
        use super::volumes::Cell;
        if let Cell::Fill(fill) = treatment {
            if matches!(
                fill.replace,
                crate::data::excavations::effects::MaterialFilter::Any
            ) {
                return CaveCut::Fill(fill.block);
            }
        }
        let cut = self.cut_unsealed(c, y, gate, interior);
        // The block the cave alone would leave here, for a field's filters.
        let natural = match cut {
            CaveCut::Open => {
                if self.aquifer_at_col(c, y).is_some() {
                    Block::Water.id()
                } else {
                    Block::Air.id()
                }
            }
            CaveCut::Barrier(block) => block,
            _ => Block::Stone.id(),
        };
        match treatment {
            Cell::Fill(fill) => {
                if fill.replace.accepts(natural) {
                    return CaveCut::Fill(fill.block);
                }
            }
            Cell::Surface => {
                if let Some(seal) = c.lat.volumes.seal_at([c.x, y, c.z]) {
                    if seal.replace.accepts(natural) {
                        return CaveCut::Fill(seal.block);
                    }
                }
                if cut == CaveCut::Solid {
                    return CaveCut::Shell;
                }
            }
            Cell::Untouched => {}
        }
        if cut != CaveCut::Open {
            return cut;
        }
        let pos = [c.x, y, c.z];
        let Some(aquifer) = self.aquifer_at_col(c, y) else {
            return cut;
        };
        if needs_barrier(pos, |neighbor| self.aquifer_at(c.lat, neighbor).is_some()) {
            CaveCut::Barrier(aquifer.barrier)
        } else {
            cut
        }
    }
}

// Water propagates laterally and downward; the open top remains its surface.
fn needs_barrier([x, y, z]: [i32; 3], wet: impl Fn([i32; 3]) -> bool) -> bool {
    [
        [x - 1, y, z],
        [x + 1, y, z],
        [x, y, z - 1],
        [x, y, z + 1],
        [x, y - 1, z],
    ]
    .into_iter()
    .any(|p| !wet(p))
}

#[cfg(test)]
mod tests;
