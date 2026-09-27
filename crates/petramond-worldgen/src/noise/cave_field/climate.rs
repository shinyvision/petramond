use super::*;
use crate::cache::local::{self, LocalTable};
use crate::cache::GenContext;
use crate::data::underground::ClimatePoint;

thread_local! {
    static HORIZONTAL: LocalTable<(GenContext, [i32; 2]), ClimatePoint> =
        LocalTable::new(&local::CAVE_CLIMATE);
}

impl CaveField {
    pub(super) fn climate_column(&self, x: i32, z: i32) -> ClimatePoint {
        let hash = (x as u32 as u64) ^ (z as u32 as u64).rotate_left(31) ^ self.seed as u64;
        HORIZONTAL.with(|table| {
            let key = (self.context(), [x, z]);
            table.get_or_insert_with(local::spread(hash), key, || {
                self.memos()
                    .climate_columns
                    .get_or_compute_unlocked(key, || self.sample_climate_column(x, z))
            })
        })
    }

    fn sample_climate_column(&self, x: i32, z: i32) -> ClimatePoint {
        let graph = self.terrain.graph();
        let point = SamplePoint::new(x as f64, 0.0, z as f64);
        let nodes = [
            channels::TEMPERATURE,
            channels::HUMIDITY,
            channels::CONTINENTALITY,
            channels::EROSION,
            channels::VARIANCE,
            channels::BASE_HEIGHT,
        ]
        .map(|channel| {
            graph
                .channel_node(channel)
                .expect("terrain climate channel")
        });
        graph.evaluate_nodes_cached(nodes, point, &mut graph.evaluation_cache())
    }

    pub(super) fn climate_at_corner(&self, x: i32, y: i32, z: i32) -> ClimatePoint {
        let mut climate = self.climate_column(x, z);
        climate[5] = (climate[5] - y as f64) / 128.0;
        climate
    }
}
