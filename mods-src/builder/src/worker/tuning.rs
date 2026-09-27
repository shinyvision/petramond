pub mod window {
    pub const WINDOW: usize = 96;
    pub const SCAN: usize = 8192;
    pub const STANCE_SEARCHES: usize = 12;
    pub const STANCE_NEARBY: u32 = 12;
    pub const SPOTS: usize = 3;
    pub const FOCUS_REACH: i32 = 8;
    pub const SEARCHES: usize = 6;
    pub const HERE: usize = 16;
    pub const LATE_CLEAR: i32 = 6;
    pub const LAYER_BAND: i32 = 5;
    pub const STAY_REACH: i32 = 12;
    pub const PERCH_REACH: i32 = 8;
    pub const DUE_CHAIN: usize = 48;
    pub const ALOFT_CANDIDATES: usize = 12;
    pub const WEIGHED: usize = 40;
    pub const LOOKAHEAD: usize = 1500;
}

pub mod price {
    pub const MOVE_TICKS: i32 = 5;
    pub const LEVEL_TICKS: i32 = 30;
    pub const MOUNT_TICKS: i32 = 20;
    pub const WALKWAY_TICKS: i32 = 35;
    pub const LEVEL_MOVES: i32 = LEVEL_TICKS / MOVE_TICKS;
    pub const MOUNT_MOVES: i32 = MOUNT_TICKS / MOVE_TICKS;
    pub const UNWALKED: i32 = 64;
    pub const DOOR_MOVES: u32 = 2;
    pub const DESIGN_MOVES: u32 = 12;
    pub const HURT_MOVES: u32 = 6;
    pub const SAFE_FALL: i32 = 3;
    pub const HAND_SECONDS: f32 = 2.5;
    pub const FRUITLESS: f32 = 4.0;
}

pub mod patience {
    pub const NOWHERE_STANDS: u64 = 100;
    pub const NOTHING_LANDED: u64 = 1200;
    pub const BAND_PATIENCE: u64 = 900;
    pub const GLAZE_PATIENCE: u64 = NOTHING_LANDED;
    pub const DUE_PATIENCE: u64 = NOTHING_LANDED;
    pub const SITE_STALLED: u64 = 12_000;
    pub const CUTTER_ROUNDS: u8 = 4;
    pub const BURY_ROUNDS: u8 = 12;
    pub const FACELESS_TRIES: u8 = 3;
    pub const SCAFFOLD_TRIES: u8 = 4;
    pub const RAISES: u8 = 12;
    pub const CLIMB_STALL: u64 = 400;
    pub const DESCENT_STALL: u64 = 400;
    pub const WALKWAY_PATIENCE: u64 = 1200;
    pub const WALKWAY_STEP_UNTAKEN: u64 = 60;
    pub const WALKWAY_SUPPORT_UNLANDED: u64 = 20;
    pub const ASTRAY_MOVES: i32 = 5;
    pub const OFF_ROUTE_TICKS: u64 = 30;
    pub const WALK_STALL: u64 = 160;
    pub const WALK_STALLS: u8 = 3;
    pub const CENTRE_TICKS: u64 = 12;
    pub const CHEST_CENTRE_TICKS: u64 = 40;
    pub const AWAIT_TICKS: u64 = 40;
    pub const GAZE_TRIES: u32 = 40;
    pub const USE_PATIENCE: u64 = 30;
    pub const WAY_IN_PATIENCE: u64 = 3000;
    pub const DOOR_TRIES: u8 = 2;
    pub const WAY_OUT_PATIENCE: u64 = 1200;
    pub const WAY_OUT_PLANS: u32 = 24;
    pub const WAY_OUT_TOTAL_PLANS: u32 = 400;
    pub const RISE_TICKS: u64 = 60;
    pub const HOP_TICKS: u64 = 80;
    pub const IDLE_NOTE: u64 = 200;
    pub const THINK_AFTER: u64 = 40;
    pub const STUCK_AFTER: u64 = 6000;
}

pub mod waits {
    pub const SEALED_WAIT: u64 = 600;
    pub const SCAFFOLD_WAIT: u64 = 100;
    pub const TOOL_TRIP_WAIT: u64 = 1200;
    pub const BURY_WAIT: u64 = 40;
    pub const STRANDING_STANCE: u64 = 20;
    pub const FOOT_UNWALKED: u64 = 100;
    pub const OUT_OF_REACH: u64 = 400;
    pub const WALK_FAILED: u64 = 200;
    pub const IDLE_CLIMB: u64 = 600;
    pub const UNREAD: u64 = 40;
    pub const UNLOADED: u64 = 200;
    pub const REFUSED: u64 = 1200;
    pub const FACELESS_SPACING: u64 = 200;
    pub const DOOR_AGAIN: u64 = 600;
    pub const DOOR_REST: u64 = 600;
    pub const DIG_REST: u64 = 400;
}

pub mod every {
    pub const MEND_EVERY: u64 = 100;
    pub const SEARCH_EVERY: u64 = 40;
    pub const UNWEDGE_EVERY: u64 = 20;
    pub const HELD_EVERY: u64 = 40;
    pub const HOME_CHECK_EVERY: u64 = 200;
    pub const WAYS_EVERY: u64 = 100;
    pub const DOOR_SCAN_EVERY: u64 = 100;
    pub const DIG_SCAN_EVERY: u64 = 20;
}

pub mod reach {
    pub const SITE_AROUND: [i32; 3] = [16, 16, 16];
    pub const SITE_ABOVE: i32 = 4;
    pub const PILLAR_SPAN: i32 = 20;
    pub const TRIM_REACH: i32 = 3;
    pub const COVER: i32 = 2;
    pub const MAX_HEIGHT: i32 = 32;
    pub const RAISE_LEVELS: i32 = 3;
    pub const RAISE_ASKS: usize = 12;
    pub const COLUMNS: usize = 12;
    pub const PILLAR_ROUTES: usize = 4;
    pub const ONWARD_STANCES: usize = 4;
    pub const ONWARD_TOPS: usize = 8;
    pub const ONWARD_SPAN: i32 = 16;
    pub const WALKWAY_LENGTH: i32 = 8;
    pub const WALKWAY_CHAIN: usize = 24;
    pub const FOOTHOLDS: usize = 64;
    pub const NEAR_WORK: usize = 32;
    pub const STANCE_ROUTES: usize = 5;
    pub const CHEST_REACH: f64 = crate::geometry::PLAN_REACH - 1.0;
    pub const SUPPORT_DEPTH: i32 = 6;
    pub const POCKET_REGION: usize = 48;
    pub const OUT_AROUND: i32 = 6;
    pub const OUT_ABOVE: i32 = 10;
    pub const OUT_BELOW: i32 = 24;
    pub const PILLAR_BACK: i32 = 12;
    pub const DOOR_REACH: i32 = 16;
    pub const SHUT_OUT: usize = 64;
    pub const SCAN_RESERVE: u32 = 12_000;
    pub const DOOR_PROBES: usize = 2;
    pub const DIG_AROUND: i32 = 7;
    pub const DIG_ABOVE: i32 = 5;
    pub const DIG_BELOW: i32 = 2;
    pub const NO_GO: usize = 64;
}

pub mod route {
    pub const MEMORY: u64 = 40;
    pub const VERDICT_MEMORY: u64 = 200;
    pub const NODES: u32 = 4000;
    pub const TRAIL: usize = 48;
    pub const NEAR: i32 = 12;
    pub const HUBS: usize = 3;
    pub const REGION_NODES: u32 = 12_000;
    pub const LEG: i32 = 12;
    pub const SHORT: u32 = 600;
    pub const SITE_SETTLE_TICKS: u64 = 60;
}

pub mod hands {
    pub const AIM_TICKS: std::ops::RangeInclusive<u64> = 5..=15;
    pub const AIM_SETTLE_TICKS: u64 = 3;
    pub const TURN_PER_TICK: f32 = 0.3;
    pub const SWING: u64 = 8;
    pub const LID_UP: u64 = 8;
    pub const LINGER: u64 = 8;
    pub const DIG_ROOM: usize = 2;
    pub const DIG_ROOM_MOST: usize = 10;
    pub const SPOIL_PER_SLOT: usize = 48;
    pub const SCAFFOLD_STOCK: u32 = 64;
}

pub mod body {
    pub const JUMP: f32 = 10.0;
    pub const RISE_JUMP: f32 = 8.0;
    pub const HOP_LAUNCH: f32 = 5.0;
    pub const HOP_SPEED: f32 = 4.0;
    pub const PERCH_OFF_CENTRE: f64 = 0.1;
    pub const STANCE_OFF_CENTRE: f64 = 0.12;
    pub const CHEST_OFF_CENTRE: f64 = 0.2;
    pub const ON_COLUMN: f64 = 0.9;
    pub const PERCH_SETTLE_TICKS: u64 = 40;
    pub const COURSE_SETTLE_TICKS: u64 = 8;
    pub const EMERGE_TICKS: u32 = 60;
    pub const BURROW_TICKS: u32 = 50;
    pub const BURROW_DEPTH: f64 = 1.7;
    pub const TRAVEL_DEPTH: f64 = 4.0;
    pub const TRAVEL_STEP: f64 = 12.0;
}

pub mod mark {
    pub const MARK_SIZE: f32 = 0.5;
    pub const MARK_CLEAR: f64 = 0.45;
    pub const MARK_BOB: [f32; 2] = [0.06, 4.0];
    pub const MARK_STEP: f32 = 0.05;
}
