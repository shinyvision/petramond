//! River carver.
//!
//! Strata P2: ports the carve parameters from `build_column` — `carve` when
//! `river_strength > 0.05`, with a bed at `(SEA_LEVEL-2).max(surf-4)`. The
//! actual channel cut stays woven into the driver's column cascade (exactly as
//! the god file had it) so output is byte-parity. P4 reshapes carvers into a
//! genuine post-fill void pass that also writes into the chunk border.

use crate::chunk::SEA_LEVEL;
use super::{CarvePlan, Carver};

pub struct RiverCarver;

impl Carver for RiverCarver {
    #[inline]
    fn plan(&self, river: f32, surf: i32) -> CarvePlan {
        CarvePlan {
            carve: river > 0.05,
            river_bed_y: (SEA_LEVEL - 2).max(surf - 4),
        }
    }
}
