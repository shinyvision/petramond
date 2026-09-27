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

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct GenFeatureFilter {
    pub min_y: i32,
    pub max_y: i32,
    pub surface_offsets: Option<[i32; 2]>,
    pub needs_blocks: bool,
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
    };

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

    pub fn is_valid(self) -> bool {
        self.min_y <= self.max_y && self.surface_offsets.is_none_or(|[lo, hi]| lo <= hi)
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
