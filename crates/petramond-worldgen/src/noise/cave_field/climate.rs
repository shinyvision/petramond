use super::*;
use crate::data::underground::ClimatePoint;

impl CaveField {
    #[inline]
    pub(super) fn climate_column(&self, x: i32, z: i32) -> ClimatePoint {
        self.columns.at(x, z)
    }

    pub(super) fn climate_at_corner(&self, x: i32, y: i32, z: i32) -> ClimatePoint {
        let mut climate = self.climate_column(x, z);
        climate[5] = (climate[5] - y as f64) / 128.0;
        climate
    }
}
