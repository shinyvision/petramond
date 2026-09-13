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

    /// The block an OPEN cell at `pos` generates as: a habitat aquifer's
    /// fluid, a pool's fluid, air otherwise. Positional, so a barrier
    /// decision can read its neighbours across box edges.
    #[inline]
    pub(super) fn open_fill(&self, lat: &CaveLattice, pos: [i32; 3]) -> u16 {
        match self.aquifer_at(lat, pos) {
            Some(aquifer) => aquifer.fluid,
            None => lat.pools.fluid_at(pos),
        }
    }

    pub(super) fn cut_from_col(&self, c: &mut Col, y: i32, gate: bool, interior: bool) -> CaveCut {
        let treatment = c.lat.volumes.at([c.x, y, c.z]);
        self.cut_from_col_treated(c, y, gate, interior, treatment)
    }

    /// [`Self::cut_from_col`] with the cell's positioned-field treatment
    /// already looked up. An open cell holding a fluid answers
    /// `CaveCut::Fill`, or the aquifer barrier sealing it.
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
        let aquifer = cut.is_air().then(|| self.aquifer_at_col(c, y)).flatten();
        // The block the cave alone would leave here, for a field's filters.
        let natural = match (cut, aquifer) {
            (CaveCut::Air, Some(aquifer)) => aquifer.fluid,
            (CaveCut::Air, None) => c.lat.pools.fluid_at([c.x, y, c.z]),
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
        if !cut.is_air() || natural == Block::Air.id() {
            return cut;
        }
        // An aquifer is a LEVEL laid over the cave, so it is sealed wherever it
        // crosses open rock; a pool is sealed by the very rock that chose it.
        if let Some(aquifer) = aquifer {
            let pos = [c.x, y, c.z];
            if needs_barrier(pos, |n| self.open_fill(c.lat, n) == aquifer.fluid) {
                return CaveCut::Barrier(aquifer.barrier);
            }
        }
        CaveCut::Fill(natural)
    }
}

// A fluid propagates laterally and downward; the open top remains its surface.
pub(super) fn needs_barrier([x, y, z]: [i32; 3], wet: impl Fn([i32; 3]) -> bool) -> bool {
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
