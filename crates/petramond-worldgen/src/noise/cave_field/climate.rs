use std::cell::RefCell;

use super::*;
use crate::data::underground::ClimatePoint;
use crate::memo::SharedMemo;

const ENTRIES: usize = 2048;
static SHARED_COLUMNS: std::sync::LazyLock<SharedMemo<(u32, i32, i32), ClimatePoint>> =
    std::sync::LazyLock::new(|| SharedMemo::new(32_768));

#[derive(Clone, Copy)]
struct Entry {
    seed: u32,
    position: [i32; 2],
    /// The last component is surface height until the caller supplies Y.
    values: ClimatePoint,
}

thread_local! {
    static HORIZONTAL: RefCell<Box<[Option<Entry>]>> = RefCell::new(vec![None; ENTRIES].into_boxed_slice());
}

impl CaveField {
    pub(super) fn climate_column(&self, x: i32, z: i32) -> ClimatePoint {
        let hash = ((x as u32 as u64) ^ (z as u32 as u64).rotate_left(31) ^ self.seed as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let slot = (hash >> (64 - ENTRIES.trailing_zeros())) as usize;
        HORIZONTAL.with(|cache| {
            if let Some(entry) = cache.borrow()[slot] {
                if entry.seed == self.seed && entry.position == [x, z] {
                    return entry.values;
                }
            }
            let values = SHARED_COLUMNS.get_or_compute_unlocked((self.seed, x, z), || {
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
            });
            cache.borrow_mut()[slot] = Some(Entry {
                seed: self.seed,
                position: [x, z],
                values,
            });
            values
        })
    }

    pub(super) fn climate_at_corner(&self, x: i32, y: i32, z: i32) -> ClimatePoint {
        let mut climate = self.climate_column(x, z);
        climate[5] = (climate[5] - y as f64) / 128.0;
        climate
    }
}
