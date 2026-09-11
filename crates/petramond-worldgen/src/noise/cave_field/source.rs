//! Completed lattice corners are positional facts, including excavation influence.
//! Share them across workers; keep repeated reads within a batch local.

use std::cell::RefCell;
use std::sync::LazyLock;

use super::{CaveField, Fields};
use crate::memo::SharedMemo;
use crate::noise::cave_density::Sample;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    seed: u32,
    tables: [usize; 2],
    position: [i32; 3],
    depth: u64,
    interior: bool,
}

type Entry = Option<(Key, Sample)>;
thread_local! {
    static LOCAL: RefCell<Box<[Entry]>> = RefCell::new(vec![None; 16384].into_boxed_slice());
}
static SHARED: LazyLock<SharedMemo<Key, Sample>> = LazyLock::new(|| SharedMemo::new(262_144));

impl CaveField {
    pub(super) fn source_sample(
        &self,
        position: [i32; 3],
        depth: f64,
        fields: Fields,
        excavation: impl FnOnce() -> (f64, f64),
    ) -> Sample {
        let key = Key {
            seed: self.seed,
            tables: [
                std::ptr::from_ref(self.underground) as usize,
                if fields.excavations {
                    std::ptr::from_ref(self.excavations) as usize
                } else {
                    0
                },
            ],
            position,
            depth: depth.to_bits(),
            interior: fields.interior,
        };
        let hash = (position[0] as u32 as u64)
            ^ (position[1] as u32 as u64).rotate_left(21)
            ^ (position[2] as u32 as u64).rotate_left(42)
            ^ self.seed as u64;
        let slot = (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 50) as usize;
        LOCAL.with(|cache| {
            if let Some((saved, sample)) = cache.borrow()[slot] {
                if saved == key {
                    return sample;
                }
            }
            let sample = SHARED.get_or_compute_unlocked(key, || {
                self.natural.sample(
                    position.map(f64::from),
                    depth,
                    excavation(),
                    fields.interior,
                )
            });
            cache.borrow_mut()[slot] = Some((key, sample));
            sample
        })
    }
}
