use super::{CaveField, Fields};
use crate::cache::local::{self, LocalTable};
use crate::cache::GenContext;
use crate::noise::cave_density::Sample;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Key {
    context: GenContext,
    position: [i32; 3],
    depth: u64,
    interior: bool,
}

thread_local! {
    static LOCAL: LocalTable<Key, Sample> = LocalTable::new(&local::CAVE_SOURCE);
}

impl CaveField {
    pub(super) fn source_sample(
        &self,
        position: [i32; 3],
        depth: f64,
        fields: Fields,
        excavation: impl FnOnce() -> (f64, f64),
    ) -> Sample {
        let key = Key {
            context: if fields.excavations {
                self.context()
            } else {
                self.context().without_excavations()
            },
            position,
            depth: depth.to_bits(),
            interior: fields.interior,
        };
        let hash = (position[0] as u32 as u64)
            ^ (position[1] as u32 as u64).rotate_left(21)
            ^ (position[2] as u32 as u64).rotate_left(42)
            ^ self.seed as u64;
        LOCAL.with(|table| {
            table.get_or_insert_with(local::spread(hash), key, || {
                self.memos().source.get_or_compute_unlocked(key, || {
                    self.natural.sample(
                        position.map(f64::from),
                        depth,
                        excavation(),
                        fields.interior,
                    )
                })
            })
        })
    }
}
