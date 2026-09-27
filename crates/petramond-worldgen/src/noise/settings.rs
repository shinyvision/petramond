use petramond_world::chunk::{SEA_LEVEL, SECTION_SIZE, WORLD_MIN_Y};

pub const CAVE_LATTICE_STEP: i32 = 4;
pub const CAVE_MIN_Y: i32 = WORLD_MIN_Y + SECTION_SIZE as i32;
pub const CAVE_FLOOR_FADE: f64 = 12.0;
pub const CAVE_SURFACE_BUFFER: i32 = 7;
pub const CAVE_ENTRANCE_MAX_DEPTH: i32 = 34;
pub const CAVE_ENTRANCE_MIN_SURFACE_Y: i32 = SEA_LEVEL + 3;
