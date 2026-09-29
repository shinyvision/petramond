use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerrainSpace {
    Air,
    Fluid,
    Solid,
}

pub const STRUCTURE_PROBES_MAX: usize = 4096;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StructureRequirementData {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub space: TerrainSpace,
}

impl StructureRequirementData {
    pub fn cell_count(&self) -> Option<usize> {
        (0..3).try_fold(1usize, |volume, axis| {
            let length = i64::from(self.max[axis]) - i64::from(self.min[axis]) + 1;
            let count = volume.checked_mul(usize::try_from(length).ok()?)?;
            (length > 0 && count <= STRUCTURE_PROBES_MAX).then_some(count)
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StructureInfoData {
    pub bounds: [([i32; 3], [i32; 3]); 4],
    pub connectors: Vec<StructureConnectorData>,
    pub requirements: Vec<StructureRequirementData>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StructureConnectorData {
    pub name: String,
    pub kind: String,
    pub pos: [i32; 3],
    pub normal: [i32; 3],
}

/// A set of biome ids (surface or underground), a bit per id.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct BiomeSet(pub [u64; 4]);

impl BiomeSet {
    pub fn new(ids: impl IntoIterator<Item = u8>) -> BiomeSet {
        let mut set = BiomeSet::default();
        for id in ids {
            set.0[usize::from(id) >> 6] |= 1 << (id & 63);
        }
        set
    }

    #[inline]
    pub fn contains(&self, id: u8) -> bool {
        self.0[usize::from(id) >> 6] & (1 << (id & 63)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }

    /// Whether any of `ids` is in the set.
    pub fn contains_any(&self, ids: impl IntoIterator<Item = u8>) -> bool {
        ids.into_iter().any(|id| self.contains(id))
    }

    /// Whether the set shares an id with `bits` (another set's words).
    #[inline]
    pub fn intersects(&self, bits: &[u64; 4]) -> bool {
        (0..4).any(|w| self.0[w] & bits[w] != 0)
    }
}

/// Where a feature's underground biomes must be able to occur for a section to be dispatched:
/// within `xz` columns sideways, `down` blocks below and `up` blocks above the section. The
/// host asks the same bounded question as [`WorldgenCall::UndergroundBiomesInBox`]
/// (crate::WorldgenCall) before encoding anything, so a feature whose biome cannot own a
/// cell in reach is never called.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct UndergroundGate {
    pub biomes: BiomeSet,
    pub xz: i32,
    pub down: i32,
    pub up: i32,
}

impl UndergroundGate {
    /// The inclusive world box the gate asks about for `section`.
    pub fn around(&self, section: [i32; 3]) -> ([i32; 3], [i32; 3]) {
        let o = section.map(|v| v.saturating_mul(16));
        (
            [
                o[0].saturating_sub(self.xz),
                o[1].saturating_sub(self.down),
                o[2].saturating_sub(self.xz),
            ],
            [
                o[0].saturating_add(15 + self.xz),
                o[1].saturating_add(15 + self.up),
                o[2].saturating_add(15 + self.xz),
            ],
        )
    }
}

/// Whether an underground biome can own a cell in each world-anchored `leaf`³ cube of a box: a
/// bit per leaf over `size` leaves from leaf `min`, x fastest, then z, then y. Conservative like
/// [`WorldgenCall::UndergroundBiomesInBox`](crate::WorldgenCall): a clear bit proves absence, a
/// set bit only admits. Positions outside the box are admitted, as unknown.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct LeafMask {
    pub leaf: i32,
    pub min: [i32; 3],
    pub size: [i32; 3],
    #[serde(with = "serde_bytes")]
    pub bits: Vec<u8>,
}

impl LeafMask {
    /// A cleared mask over the leaves the world box `lo..=hi` touches.
    pub fn over(leaf: i32, lo: [i32; 3], hi: [i32; 3]) -> LeafMask {
        let min = lo.map(|v| v.div_euclid(leaf));
        let size = std::array::from_fn(|a| (hi[a].div_euclid(leaf) - min[a] + 1).max(0));
        let count = size.iter().map(|&n| n as usize).product::<usize>();
        LeafMask {
            leaf,
            min,
            size,
            bits: vec![0; count.div_ceil(8)],
        }
    }

    /// A mask admitting every position.
    pub fn everywhere() -> LeafMask {
        LeafMask::default()
    }

    /// The bit of the leaf holding `p`, when inside the box.
    #[inline]
    pub fn index(&self, p: [i32; 3]) -> Option<usize> {
        if self.leaf <= 0 {
            return None;
        }
        let l = [0, 1, 2].map(|a| p[a].div_euclid(self.leaf) - self.min[a]);
        if (0..3).any(|a| l[a] < 0 || l[a] >= self.size[a]) {
            return None;
        }
        Some(((l[1] * self.size[2] + l[2]) * self.size[0] + l[0]) as usize)
    }

    pub fn set(&mut self, index: usize) {
        self.bits[index / 8] |= 1 << (index % 8);
    }

    /// Whether the biome may own a cell in the leaf holding `p`.
    #[inline]
    pub fn may_hold(&self, p: [i32; 3]) -> bool {
        match self.index(p) {
            Some(i) => self.bits.get(i / 8).is_some_and(|b| b >> (i % 8) & 1 != 0),
            None => true,
        }
    }

    /// The leaves of the column through `[x, z]`, a bit per leaf from the box's lowest (bit 0)
    /// upward; all set outside the box or past 64 leaves, as unknown. Test a cell with
    /// [`ColumnLeaves::may_hold`] — one index per column instead of one per cell.
    pub fn column(&self, x: i32, z: i32) -> ColumnLeaves {
        let unknown = ColumnLeaves {
            bits: u64::MAX,
            leaf: 0,
            base: 0,
        };
        if self.leaf <= 0 || self.size[1] > 64 {
            return unknown;
        }
        let (lx, lz) = (
            x.div_euclid(self.leaf) - self.min[0],
            z.div_euclid(self.leaf) - self.min[2],
        );
        if lx < 0 || lx >= self.size[0] || lz < 0 || lz >= self.size[2] {
            return unknown;
        }
        let mut bits = 0u64;
        for ly in 0..self.size[1] {
            let i = ((ly * self.size[2] + lz) * self.size[0] + lx) as usize;
            if self.bits[i / 8] >> (i % 8) & 1 != 0 {
                bits |= 1 << ly;
            }
        }
        ColumnLeaves {
            bits,
            leaf: self.leaf,
            base: self.min[1],
        }
    }

    /// Whether any leaf of the column through `[x, z]` admits the biome.
    pub fn column_may_hold(&self, x: i32, z: i32) -> bool {
        self.column(x, z).bits != 0
    }

    /// Whether any leaf admits the biome.
    pub fn any(&self) -> bool {
        self.leaf <= 0 || self.bits.iter().any(|&b| b != 0)
    }

    /// Whether every leaf admits the biome (nothing to skip).
    pub fn complete(&self) -> bool {
        if self.leaf <= 0 {
            return true;
        }
        let count = self.size.iter().map(|&n| n as usize).product::<usize>();
        (0..count).all(|i| self.bits[i / 8] >> (i % 8) & 1 != 0)
    }

    pub fn is_well_formed(&self) -> bool {
        if self.leaf <= 0 {
            return self.bits.is_empty();
        }
        let count = self
            .size
            .iter()
            .try_fold(1usize, |n, &s| n.checked_mul(usize::try_from(s).ok()?));
        count.is_some_and(|c| c.div_ceil(8) == self.bits.len())
    }
}

/// One column of a [`LeafMask`]: its leaves' bits, tested per y with one shift.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColumnLeaves {
    bits: u64,
    leaf: i32,
    base: i32,
}

impl ColumnLeaves {
    #[inline]
    pub fn any(&self) -> bool {
        self.bits != 0
    }

    /// Whether the leaf at height `y` admits the biome (unknown heights admit).
    #[inline]
    pub fn may_hold(&self, y: i32) -> bool {
        if self.leaf <= 0 {
            return true;
        }
        let ly = y.div_euclid(self.leaf) - self.base;
        !(0..64).contains(&ly) || self.bits >> ly & 1 != 0
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct GenFeatureFilter {
    pub min_y: i32,
    pub max_y: i32,
    pub surface_offsets: Option<[i32; 2]>,
    pub needs_blocks: bool,
    /// Whether the dispatch carries the section's column surface heights and biomes.
    pub needs_columns: bool,
    /// Whether the feature keeps columns for itself ([`GuestCall::GenClaims`](crate::GuestCall)):
    /// the engine's trees stay out of them.
    pub claims: bool,
    /// Dispatch only for sections one of whose columns has a surface biome in the set.
    pub surface_biomes: Option<BiomeSet>,
    /// Dispatch only where one of the gate's underground biomes can own a cell in reach.
    pub underground: Option<UndergroundGate>,
}

impl Default for GenFeatureFilter {
    fn default() -> Self {
        Self::ANY
    }
}

impl GenFeatureFilter {
    pub const ANY: GenFeatureFilter = GenFeatureFilter {
        min_y: i32::MIN,
        max_y: i32::MAX,
        surface_offsets: None,
        needs_blocks: true,
        needs_columns: true,
        claims: false,
        surface_biomes: None,
        underground: None,
    };

    /// Only sections with a column in one of `ids` (surface biomes) are dispatched.
    pub fn in_surface_biomes(self, ids: impl IntoIterator<Item = u8>) -> GenFeatureFilter {
        GenFeatureFilter {
            surface_biomes: Some(BiomeSet::new(ids)),
            ..self
        }
    }

    /// Only sections within `xz`/`down`/`up` blocks of where one of `ids` (underground biomes)
    /// can own a cell are dispatched. An empty set admits nothing.
    pub fn near_underground_biomes(
        self,
        ids: impl IntoIterator<Item = u8>,
        xz: i32,
        down: i32,
        up: i32,
    ) -> GenFeatureFilter {
        GenFeatureFilter {
            underground: Some(UndergroundGate {
                biomes: BiomeSet::new(ids),
                xz,
                down,
                up,
            }),
            ..self
        }
    }

    /// Whether the section's column biomes pass the surface-biome set (always, without one).
    #[inline]
    pub fn admits_columns(&self, biomes: &[u8]) -> bool {
        self.surface_biomes
            .is_none_or(|set| set.contains_any(biomes.iter().copied()))
    }

    pub const fn y_band(min_y: i32, max_y: i32) -> GenFeatureFilter {
        GenFeatureFilter {
            min_y,
            max_y,
            ..Self::ANY
        }
    }

    pub const fn surface_band(below: i32, above: i32) -> GenFeatureFilter {
        GenFeatureFilter {
            surface_offsets: Some([below, above]),
            ..Self::ANY
        }
    }

    pub const fn without_blocks(self) -> GenFeatureFilter {
        GenFeatureFilter {
            needs_blocks: false,
            ..self
        }
    }

    /// A feature that reads neither `GenCtx::surface_y` nor `GenCtx::biome`: its dispatches
    /// skip copying and decoding the section's column data.
    pub const fn without_columns(self) -> GenFeatureFilter {
        GenFeatureFilter {
            needs_columns: false,
            ..self
        }
    }

    /// A feature that answers [`GuestCall::GenClaims`](crate::GuestCall) for the columns it
    /// keeps for itself.
    pub const fn with_claims(self) -> GenFeatureFilter {
        GenFeatureFilter {
            claims: true,
            ..self
        }
    }

    pub fn is_valid(self) -> bool {
        self.min_y <= self.max_y
            && self.surface_offsets.is_none_or(|[lo, hi]| lo <= hi)
            && self
                .underground
                .is_none_or(|g| g.xz >= 0 && g.down >= 0 && g.up >= 0)
    }

    pub fn intersects(self, cy: i32, surfaces: &[i32]) -> bool {
        let lo = i64::from(cy) * 16;
        let hi = lo + 15;
        if lo > i64::from(self.max_y) || hi < i64::from(self.min_y) {
            return false;
        }
        self.surface_offsets.is_none_or(|[below, above]| {
            surfaces.iter().any(|&y| {
                lo <= i64::from(y) + i64::from(above) && hi >= i64::from(y) + i64::from(below)
            })
        })
    }
}

#[cfg(test)]
mod tests;
