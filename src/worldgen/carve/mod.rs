//! Carvers — terrain subtraction after the solid fill.
//!
//! Strata P2: only the river carver exists, and it contributes *parameters*
//! (`CarvePlan`) that the driver's cascade consumes inline, preserving the god
//! file's exact carve branch. The `Carver` trait + `CarverSet` set up the fixed
//! carver ordering that P4 will run as true post-fill passes.

pub mod river;

use river::RiverCarver;

#[derive(Copy, Clone)]
pub struct CarvePlan {
    pub carve: bool,
    pub river_bed_y: i32,
}

pub trait Carver: Send + Sync {
    fn plan(&self, river: f32, surf: i32) -> CarvePlan;
}

pub struct CarverSet {
    river: RiverCarver,
}

impl Default for CarverSet {
    fn default() -> Self {
        Self { river: RiverCarver }
    }
}

impl CarverSet {
    /// Combined carve plan for a column. P2 has a single carver.
    #[inline]
    pub fn plan(&self, river: f32, surf: i32) -> CarvePlan {
        self.river.plan(river, surf)
    }
}
