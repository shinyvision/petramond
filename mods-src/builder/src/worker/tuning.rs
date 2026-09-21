//! Every number the golem's work is tuned by, grouped by what it tunes. A
//! quantity that more than one part of the worker reckons with is written
//! once here and the other forms are derived from it.

/// How much open work a plan weighs, and how much of it per tick.
pub mod window {
    /// How many actionable open units a plan weighs — the nearest to the golem
    /// of the layers being worked — and how far past the first open unit it scans
    /// for them.
    pub const WINDOW: usize = 96;
    pub const SCAN: usize = 8192;
    /// How many of the nearest candidates are asked for a place to stand before
    /// any is given a pillar, and how many moves past the nearest they may lie:
    /// work done from the ground a few steps on comes before a climb here.
    pub const STANCE_SEARCHES: usize = 12;
    pub const STANCE_NEARBY: u32 = 12;
    /// How many places to work from a plan weighs against each other. The
    /// place taken is the one that costs least for each block it would lay: a
    /// trip for one block loses to a short climb that lays six, and to a longer
    /// walk that lays a dozen.
    pub const SPOTS: usize = 3;
    /// How far from the corner it is working through the golem looks for its next
    /// job before it moves on to another.
    pub const FOCUS_REACH: i32 = 8;
    /// How many candidates may search for a stance in one tick, and how many are
    /// first tried from where the golem already stands.
    pub const SEARCHES: usize = 6;
    pub const HERE: usize = 16;
    /// How far from a late block (leaves) other open work must be for it to go
    /// in with whatever else is in reach.
    pub const LATE_CLEAR: i32 = 6;
    /// Layers above the lowest unfinished one that work may be taken from.
    pub const LAYER_BAND: i32 = 5;
    /// How near work left in the band the golem is in must be for it to stay in
    /// that band when lower work comes round again.
    pub const STAY_REACH: i32 = 12;
    /// How far across the ground from the golem's perch work above the band
    /// is still taken from it: a raise or a short walkway away.
    pub const PERCH_REACH: i32 = 8;
    /// How far along the neighbours of a block waiting on them for a face a
    /// face is looked for.
    pub const DUE_CHAIN: usize = 48;
    /// Open work weighed per plan from a perch, nearest first.
    pub const ALOFT_CANDIDATES: usize = 12;
    /// Open work weighed per perch search: the nearest to the block climbed for.
    pub const WEIGHED: usize = 40;
    /// Open units a batch looks ahead over.
    pub const LOOKAHEAD: usize = 1500;
}

/// What getting somewhere costs. A move walked is the unit; ticks and moves
/// are two readings of the same prices.
pub mod price {
    /// What getting to a place to work from costs in ticks: a move walked, a
    /// pillar level climbed and dug down again, getting on and off a pillar.
    pub const MOVE_TICKS: i32 = 5;
    pub const LEVEL_TICKS: i32 = 30;
    pub const MOUNT_TICKS: i32 = 20;
    /// A walkway cell laid, and later taken down again.
    pub const WALKWAY_TICKS: i32 = 35;
    /// The same prices in moves walked: a level (about six), and getting on
    /// and off a pillar.
    pub const LEVEL_MOVES: i32 = LEVEL_TICKS / MOVE_TICKS;
    pub const MOUNT_MOVES: i32 = MOUNT_TICKS / MOVE_TICKS;
    /// A foot the flood from the golem does not reach counts as a long way off.
    pub const UNWALKED: i32 = 64;
    /// Opening a door and stepping through it.
    pub const DOOR_MOVES: u32 = 2;
    /// Breaking one of the design's own blocks, besides the digging: it is laid
    /// again afterwards.
    pub const DESIGN_MOVES: u32 = 12;
    /// A point of damage a fall does.
    pub const HURT_MOVES: u32 = 6;
    /// A fall hurts a point for every block past this many.
    pub const SAFE_FALL: i32 = 3;
    /// A block's break time by hand: seconds a point of hardness, and how much
    /// slower where the hand harvests nothing. A tool of the block's kind and tier
    /// divides it by its speed.
    pub const HAND_SECONDS: f32 = 2.5;
    pub const FRUITLESS: f32 = 4.0;
}

/// How long, or how often, something may fail before it is given up.
pub mod patience {
    /// How long a stance search that found nothing stands unasked while the
    /// site and the golem stay as they were: routes that failed are forgiven
    /// with time, so it is asked again after this.
    pub const NOWHERE_STANDS: u64 = 100;
    /// How long nothing may land before the golem stops holding work back for
    /// work that may never come.
    pub const NOTHING_LANDED: u64 = 1200;
    /// How long nothing may land before work above the layer band is taken
    /// anyway.
    pub const BAND_PATIENCE: u64 = 900;
    /// How long without a block landing before windows are glazed anyway: work
    /// the rest waits on may never come. At a quarter of that the golem asks the
    /// chests whether the rest of the work can be built at all.
    pub const GLAZE_PATIENCE: u64 = NOTHING_LANDED;
    /// How long nothing may land before a block waiting on its neighbours for a
    /// face is propped up after all.
    pub const DUE_PATIENCE: u64 = NOTHING_LANDED;
    /// A build this long without a block landing is stuck on something else,
    /// and closing a gap may be all that is left.
    pub const SITE_STALLED: u64 = 12_000;
    /// How often a unit that would cut off access is weighed again before it
    /// goes in.
    pub const CUTTER_ROUNDS: u8 = 4;
    /// How often a placement waits for the digging it would bury.
    pub const BURY_ROUNDS: u8 = 12;
    /// How many spaced refusals for want of a face a unit takes before it gets
    /// scaffolding under it.
    pub const FACELESS_TRIES: u8 = 3;
    /// How often scaffolding out of reach is asked for before it is left.
    pub const SCAFFOLD_TRIES: u8 = 4;
    /// How many times one pillar is raised for work it does not see.
    pub const RAISES: u8 = 12;
    /// How long a climb may go without laying a level before it is given up.
    pub const CLIMB_STALL: u64 = 400;
    /// How long a descent may go without a level coming out before the pillar
    /// is left.
    pub const DESCENT_STALL: u64 = 400;
    /// How long a walkway may take to lay or take down before it is given up.
    pub const WALKWAY_PATIENCE: u64 = 1200;
    /// How long the next step out along a walkway may go untaken, and how long
    /// a support asked for may go without landing, before the walkway is given
    /// up.
    pub const WALKWAY_STEP_UNTAKEN: u64 = 60;
    pub const WALKWAY_SUPPORT_UNLANDED: u64 = 20;
    /// How many moves farther from its goal than it has been a walking golem may
    /// stray before it counts as off its way.
    pub const ASTRAY_MOVES: i32 = 5;
    /// How long a walking golem may stand off every way to its goal.
    pub const OFF_ROUTE_TICKS: u64 = 30;
    /// How long a walk may go without getting nearer before it is given up,
    /// and how many given up near one spot before the golem comes up at home.
    pub const WALK_STALL: u64 = 160;
    pub const WALK_STALLS: u8 = 3;
    /// How long a golem at its stance shuffles to the centre before it works
    /// from where it is, and how long one at a chest does.
    pub const CENTRE_TICKS: u64 = 12;
    pub const CHEST_CENTRE_TICKS: u64 = 40;
    /// How long a queued place or break may go unanswered.
    pub const AWAIT_TICKS: u64 = 40;
    /// Tries an action waits for the gaze to land before its stance is given up.
    pub const GAZE_TRIES: u32 = 40;
    /// How long a door in reach is looked for before it is left as it stands.
    pub const USE_PATIENCE: u64 = 30;
    /// How long the golem keeps digging toward one piece of work before it gives
    /// that way in up and weighs another.
    pub const WAY_IN_PATIENCE: u64 = 3000;
    /// How often one door is toggled as the way out before it counts as a wall:
    /// toggled open it frees the golem, or it was never the way.
    pub const DOOR_TRIES: u8 = 2;
    /// How long the golem may stand in one cell getting out, and how many plans
    /// it may make there, before it gives up and comes up at home instead.
    pub const WAY_OUT_PATIENCE: u64 = 1200;
    pub const WAY_OUT_PLANS: u32 = 24;
    /// However it moves, a way out that takes this many plans is no way out: a
    /// golem walking between two of its pillar tops reset every cell's patience.
    pub const WAY_OUT_TOTAL_PLANS: u32 = 400;
    /// How long a rise onto a scaffold, and a hop down off a ledge, may take.
    pub const RISE_TICKS: u64 = 60;
    pub const HOP_TICKS: u64 = 80;
    /// How long the golem may stand with nothing to show for it before the table
    /// says what it is waiting for.
    pub const IDLE_NOTE: u64 = 200;
    /// Standing with nothing in hand this long reads as thinking (ticks): shorter
    /// waits are the pauses between any two jobs.
    pub const THINK_AFTER: u64 = 40;
    /// Nothing laid or dug for this long is stuck: every wait the planner sets
    /// itself has run out many times over by then.
    pub const STUCK_AFTER: u64 = 6000;
}

/// How long work set aside waits before it is weighed again.
pub mod waits {
    /// How long a unit that would cut off access waits before it is weighed again.
    pub const SEALED_WAIT: u64 = 600;
    /// How long work waits for blocks to make its pillar of.
    pub const SCAFFOLD_WAIT: u64 = 100;
    /// How long after a trip for tools before another is made for the same want.
    pub const TOOL_TRIP_WAIT: u64 = 1200;
    /// How long a placement waits for the digging it would bury.
    pub const BURY_WAIT: u64 = 40;
    /// How long a task waits whose stance would wall the golem in: another
    /// stance is tried the next time round.
    pub const STRANDING_STANCE: u64 = 20;
    /// How long a task waits whose pillar's foot cannot be walked to.
    pub const FOOT_UNWALKED: u64 = 100;
    /// How long work out of every reach, or with nothing to be propped on,
    /// waits.
    pub const OUT_OF_REACH: u64 = 400;
    /// How long the task of a walk given up waits.
    pub const WALK_FAILED: u64 = 200;
    /// How long the task of a climb that did no work waits.
    pub const IDLE_CLIMB: u64 = 600;
    /// How long a placement waits on a seal check that could not be read, or
    /// on a body in its way; on ground not loaded or support not there; on a
    /// refusal nothing the golem does changes.
    pub const UNREAD: u64 = 40;
    pub const UNLOADED: u64 = 200;
    pub const REFUSED: u64 = 1200;
    /// Ticks between counted tries of a unit with nothing to be placed against.
    pub const FACELESS_SPACING: u64 = 200;
    /// How long before the same door is tried again as the way to work.
    pub const DOOR_AGAIN: u64 = 600;
    /// How long the golem leaves the question of a door in the way after a
    /// scan that found none.
    pub const DOOR_REST: u64 = 600;
    /// How long the golem leaves the question alone after one that found nothing.
    pub const DIG_REST: u64 = 400;
}

/// How often the golem looks again at what changes slowly.
pub mod every {
    /// Ticks per point of health the golem mends: stone knits slowly, but a long
    /// build's knocks and scrapes never add up to a lost golem.
    pub const MEND_EVERY: u64 = 100;
    /// How long a golem that cannot be found is looked for again.
    pub const SEARCH_EVERY: u64 = 40;
    /// How often a walking or planning golem is checked for being wedged.
    pub const UNWEDGE_EVERY: u64 = 20;
    /// How often the golem asks its hands and the chests what they hold.
    pub const HELD_EVERY: u64 = 40;
    /// How often home is checked for still being ground to stand on.
    pub const HOME_CHECK_EVERY: u64 = 200;
    /// How often the open panels are asked whether a body passes their gaps.
    pub const WAYS_EVERY: u64 = 100;
    /// How often the golem looks for a door in the way of work at all.
    pub const DOOR_SCAN_EVERY: u64 = 100;
    /// How often the golem weighs a way in through ground at all.
    pub const DIG_SCAN_EVERY: u64 = 20;
}

/// How far, how high and how many: the spans the golem's searches cover.
pub mod reach {
    /// The room around the design, and over it, that a job's walking is
    /// judged in.
    pub const SITE_AROUND: [i32; 3] = [16, 16, 16];
    pub const SITE_ABOVE: i32 = 4;
    /// How far below work a pillar's foot is looked for when pricing a climb.
    pub const PILLAR_SPAN: i32 = 20;
    /// How far around unreachable work natural overgrowth is cut away.
    pub const TRIM_REACH: i32 = 3;
    /// How much earth over work out of reach is taken off: the cover that hides
    /// it, never a shaft — the way in itself is `wayin::dig_toward`'s to find.
    pub const COVER: i32 = 2;
    /// The tallest pillar the golem builds.
    pub const MAX_HEIGHT: i32 = 32;
    /// How many levels at a time a pillar is raised for work it does not see,
    /// and how much open work is asked about.
    pub const RAISE_LEVELS: i32 = 3;
    pub const RAISE_ASKS: usize = 12;
    /// Columns examined per pillar search, and routes tried to their feet.
    pub const COLUMNS: usize = 12;
    pub const PILLAR_ROUTES: usize = 4;
    /// Stances an onward pillar is sought for, walkway ends tried as its top
    /// for each, and how far from the stance the walkway is followed.
    pub const ONWARD_STANCES: usize = 4;
    pub const ONWARD_TOPS: usize = 8;
    pub const ONWARD_SPAN: i32 = 16;
    /// The longest walkway laid in one go.
    pub const WALKWAY_LENGTH: i32 = 8;
    /// The longest a chain of walkways laid on from one another gets: every cell
    /// of it is walked back and taken down.
    pub const WALKWAY_CHAIN: usize = 24;
    /// Candidates tested for standing room per stance search: those nearest the
    /// golem, then those nearest the work.
    pub const FOOTHOLDS: usize = 64;
    pub const NEAR_WORK: usize = 32;
    /// Candidates routed to per stance search.
    pub const STANCE_ROUTES: usize = 5;
    /// How close a stance must be to a container to be walked to: well inside
    /// reach, so the golem ends up beside the chest rather than at arm's length
    /// across a diagonal.
    pub const CHEST_REACH: f64 = crate::geometry::PLAN_REACH - 1.0;
    /// How deep under a floating cell the ground may lie.
    pub const SUPPORT_DEPTH: i32 = 6;
    /// The most open cells a region may hold and still count as closed off:
    /// past this it is a room or the open air.
    pub const POCKET_REGION: usize = 48;
    /// How far around the golem a way out is looked for.
    pub const OUT_AROUND: i32 = 6;
    pub const OUT_ABOVE: i32 = 10;
    pub const OUT_BELOW: i32 = 24;
    /// How far a stranded golem looks for its own pillar to climb down.
    pub const PILLAR_BACK: i32 = 12;
    /// How far from the work a door may stand and still be its way in.
    pub const DOOR_REACH: i32 = 16;
    /// How much of the work already found shut out one scan answers for.
    pub const SHUT_OUT: usize = 64;
    /// What the tick's route budget must still hold for a scan to start: a scan
    /// that spends it leaves the planner answering Busy, which reads as out of
    /// reach.
    pub const SCAN_RESERVE: u32 = 12_000;
    /// How many doors a scan asks about, and how many ways past each: route
    /// answers come out of the tick's own budget, and a scan that spends it all
    /// leaves the planner nothing to find a stance with.
    pub const DOOR_PROBES: usize = 2;
    /// How far around work walled in by ground its way in is weighed. Above
    /// it, room enough for a stance beside and over the work.
    pub const DIG_AROUND: i32 = 7;
    pub const DIG_ABOVE: i32 = 5;
    /// Barely below the work: a builder cuts in at the level it lays, not a tunnel
    /// under the floor. Two, so the floor under a level stance is weighed.
    pub const DIG_BELOW: i32 = 2;
    /// How many refused steps are remembered before the oldest is forgotten.
    pub const NO_GO: usize = 64;
}

/// The route searches' budgets and memories.
pub mod route {
    /// How long a route answer is trusted: long enough for a search to work
    /// through its candidates over several ticks, short against the world's pace.
    pub const MEMORY: u64 = 40;
    /// How long a failed round trip is skipped without searching again: most of a
    /// build's dead ends (an upper floor with no way up yet) stay dead a while.
    pub const VERDICT_MEMORY: u64 = 200;
    /// Expansions one search may spend: the navigator's own route budget.
    pub const NODES: u32 = 4000;
    /// Trail cells kept, how near the target one must be to stand in for home,
    /// and how many are asked.
    pub const TRAIL: usize = 48;
    pub const NEAR: i32 = 12;
    pub const HUBS: usize = 3;
    /// Footholds a flood may find: every walkable cell of a site with room
    /// around it, well inside one tick's route budget.
    pub const REGION_NODES: u32 = 12_000;
    /// How far one leg of a long walk reaches: a single search decides it.
    pub const LEG: i32 = 12;
    /// Expansions a search spends where the site's flood decides what it leaves
    /// undecided: a walk that is there is found within a few hundred, and one
    /// that is not costs the flood a fraction of a search run to its end.
    pub const SHORT: u32 = 600;
    /// Ticks the whole site must have stood loaded before a shut way home is
    /// believed: route answers remembered from while it loaded (ground not in
    /// yet reads as open air to them) have run out by then.
    pub const SITE_SETTLE_TICKS: u64 = 60;
}

/// The pace of the hands, and what they carry.
pub mod hands {
    /// A hand never moves the instant it could: consecutive placements are a few
    /// ticks apart, different each time. Time spent walking or climbing since the
    /// last one counts; the head still takes a moment to settle on the cell.
    pub const AIM_TICKS: std::ops::RangeInclusive<u64> = 5..=15;
    pub const AIM_SETTLE_TICKS: u64 = 3;
    /// How fast the body comes round to a block behind it, in radians a tick
    /// (the golem row's `turn_rate`): a placement waits out the turn it asks for.
    pub const TURN_PER_TICK: f32 = 0.3;
    /// Ticks a head takes to swing right round and settle.
    pub const SWING: u64 = 8;
    /// Ticks a chest lid takes to lift before a hand goes in, and how long the
    /// golem lingers at it after.
    pub const LID_UP: u64 = 8;
    pub const LINGER: u64 = 8;
    /// Carried slots a fetched batch leaves free, besides the tools', for what
    /// digging collects.
    pub const DIG_ROOM: usize = 2;
    /// The most slots kept free for it, and the blocks of spoil one slot is
    /// reckoned to hold (a stack, less what odd kinds of spoil take up).
    pub const DIG_ROOM_MOST: usize = 10;
    pub const SPOIL_PER_SLOT: usize = 48;
    /// Scaffold blocks a golem sets out with, one slot's worth: a tall pillar and
    /// a walkway out from its top. What it digs back out it lays again.
    pub const SCAFFOLD_STOCK: u32 = 64;
}

/// How the body moves and how exactly it must stand.
pub mod body {
    /// Upward launch speed up a pillar: clears two cells, so one jump places
    /// two levels.
    pub const JUMP: f32 = 10.0;
    /// Upward launch that lifts the feet a little over one block.
    pub const RISE_JUMP: f32 = 8.0;
    /// The launch and the speed of a hop down off a ledge.
    pub const HOP_LAUNCH: f32 = 5.0;
    pub const HOP_SPEED: f32 = 4.0;
    /// How far off its cell's centre a body may stand and still judge what it
    /// sees from a perch, or jump up its column.
    pub const PERCH_OFF_CENTRE: f64 = 0.1;
    /// How far off its cell's centre a body may stand and still count as
    /// centred at a stance.
    pub const STANCE_OFF_CENTRE: f64 = 0.12;
    /// How far off its cell's centre a body may stand and still reach into a
    /// chest as its cell was judged to.
    pub const CHEST_OFF_CENTRE: f64 = 0.2;
    /// How far from its column's centre a body on a pillar may be nudged and
    /// still stand on it.
    pub const ON_COLUMN: f64 = 0.9;
    /// How long a golem just up a pillar, or just walked along its course,
    /// settles onto its centre before judging what it sees from there.
    pub const PERCH_SETTLE_TICKS: u64 = 40;
    pub const COURSE_SETTLE_TICKS: u64 = 8;
    pub const EMERGE_TICKS: u32 = 60;
    pub const BURROW_TICKS: u32 = 50;
    /// How far below its feet cell the golem starts and ends.
    pub const BURROW_DEPTH: f64 = 1.7;
    /// How far below the lower of where it sinks and where it rises the golem
    /// travels between them, and how far it moves in a tick: a body is placed at
    /// most a few blocks from where it was.
    pub const TRAVEL_DEPTH: f64 = 4.0;
    pub const TRAVEL_STEP: f64 = 12.0;
}

/// The mark over the golem's head.
pub mod mark {
    /// The mark's width in blocks, and how far over the head its centre hangs.
    pub const MARK_SIZE: f32 = 0.5;
    pub const MARK_CLEAR: f64 = 0.45;
    /// A slow, gentle rise and fall: blocks either side of rest, and seconds a
    /// cycle.
    pub const MARK_BOB: [f32; 2] = [0.06, 4.0];
    /// A mark hung beside the head follows the body in steps this fine.
    pub const MARK_STEP: f32 = 0.05;
}
