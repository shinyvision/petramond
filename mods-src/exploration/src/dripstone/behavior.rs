//! The spike rows' block behaviour: what a stalactite or stalagmite DOES.
//!
//! The engine keeps the shape (a run's taper is refined on every edit); the
//! pack keeps the consequences. Both rows share one behaviour key, and every
//! hook re-reads the cell's block first, so a hook that fires on a cell the
//! world has already changed under it does nothing.

use mod_sdk::*;

use super::{Dripstone, Run, MAX_RUN, PLACED_KEY, POINTED_ITEM};

// A cell is random-ticked about once every 68 seconds (the engine draws
// `RANDOM_TICK_SPEED` = 3 of a section's 4,096 cells per tick, 20 ticks a
// second), so a per-mille chance here is a wait of `68 s / chance`. The
// first cut's 12 per mille was a growth step every 95 MINUTES — rare enough
// that a player would call it broken.

/// Per-mille chance per random tick that a dripping tip GROWS. ~7.6 minutes
/// per step; when both directions are open each is half that, so a run and
/// the stalagmite under it take their turns and a farm is a patient thing
/// rather than an invisible one.
const GROW_PER_MILLE: u64 = 150;
/// Per-mille chance per random tick that a drip fills the vessel under it:
/// ~4.5 minutes a fill, a water source worth walking back to.
const VESSEL_PER_MILLE: u64 = 250;
/// How far below a tip a drip reaches: the first non-air cell within this
/// many is what it lands on.
const DRIP_REACH: i32 = 11;
/// Downward launch speed of a falling piece, m/s. Gravity does the rest;
/// this only keeps the pieces from hanging for a tick where they were.
const FALL_SPEED: f32 = 2.0;

pub fn on_hook(d: &Dripstone, kind: BlockHookKind, pos: [i32; 3]) {
    let Some(block) = get_block(pos) else {
        return;
    };
    if !d.is_spike(block) {
        return;
    }
    match kind {
        BlockHookKind::NeighborUpdate => {
            if !held(d, pos, block) {
                come_down(d, pos, block);
            } else {
                // The cheapest chance to tell the truth: whatever changed
                // beside this spike may have started or stopped its drip, and
                // a tip newly exposed by the segment below it breaking has a
                // stale skin to shed.
                wetness(d, pos, block);
            }
        }
        BlockHookKind::RandomTick => grow(d, pos, block),
        BlockHookKind::ScheduledTick => {}
    }
}

/// Bring a hanging TIP's row in line with whether it is really dripping, and
/// answer that verdict. `None` = not a tip of a hanging run, so there is
/// nothing to say.
///
/// This is the one place the drip's MEANING lives. A particle emitter is
/// static row data, so "is dripping" has to be block identity (the cauldron's
/// fill-state pattern), and the swap carries the cell's KV and refined state
/// across — a plain write would clear the cultivated mark and quietly un-farm
/// the run. The emitter's own `requires_open: below` keeps the drip to the
/// free end, so a wet segment that later gets buried shows nothing without
/// anyone tidying it up.
fn wetness(d: &Dripstone, pos: [i32; 3], block: BlockId) -> Option<bool> {
    if d.run_of(block) != Some(Run::Hanging) {
        return None;
    }
    if d.segment_at([pos[0], pos[1] - 1, pos[2]], Run::Hanging) {
        return None; // not the free end
    }
    let (_, support) = d.run_root(pos, Run::Hanging);
    let wet = drips(d, support);
    let want = if wet { d.stalactite_wet } else { d.stalactite };
    if block != want {
        swap_block(pos, want);
    }
    Some(wet)
}

/// Whether the cell rootward still holds this segment: another segment of
/// the same run, or a full-cube surface (the same footing the engine's
/// placement demanded). An unloaded rootward cell counts as held — a
/// streaming edge must never bring a run down.
fn held(d: &Dripstone, pos: [i32; 3], block: BlockId) -> bool {
    let Some(run) = d.run_of(block) else {
        return true;
    };
    let s = [pos[0], pos[1] + run.root_step(), pos[2]];
    match get_block(s) {
        // Another segment of the same run — wet or dry, it is one run.
        Some(b) if d.run_of(b) == Some(run) => true,
        Some(_) => matches!(collision_shape_at(s), Some(CollisionShape::Full)),
        None => true,
    }
}

/// THIS SEGMENT leaves the world — one cell, never a run.
///
/// Removing it announces the change to its six neighbours, so the segment
/// tipward of it takes a `NeighborUpdate` on the next tick, finds nothing
/// holding it, and comes down in turn: the run unzips one cell per tick
/// through the engine's block-update cascade, exactly as a hanging vine
/// curtain does. Walking the run and clearing it here would be a SECOND way
/// to sequence the same break — faster than the cascade, so it raced it and
/// took the whole run in one tick.
///
/// A stalactite FALLS: its segment becomes a flying piece of the item,
/// seated at its own cell, so an unzipping run rains its pieces down in
/// order. A stalagmite has nowhere to fall and simply crumbles into a drop.
fn come_down(d: &Dripstone, pos: [i32; 3], block: BlockId) {
    if !set_block(pos, d.air) {
        return;
    }
    let centre = [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.5,
        pos[2] as f32 + 0.5,
    ];
    if d.run_of(block) == Some(Run::Hanging) {
        launch_item(POINTED_ITEM, centre, [0.0, -FALL_SPEED, 0.0], None, &[]);
    } else {
        spawn_item(POINTED_ITEM, 1, centre);
    }
}

/// A DRIPPING stalactite tip — water standing over the dripstone block its
/// run hangs from — does one of three things on a random tick: fills the
/// vessel its drip lands in, extends itself by a segment, or raises the
/// stalagmite under it (starting one on a full surface). Stalagmites never
/// act on their own.
fn grow(d: &Dripstone, pos: [i32; 3], block: BlockId) {
    // The skin comes first and is its own answer: the row a tip wears IS
    // whether it drips, so the particle a player sees and the growth that
    // follows can never disagree.
    if wetness(d, pos, block) != Some(true) {
        return;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    let (len, _) = d.run_root(pos, Run::Hanging);
    // What the drip lands on: the first non-air cell within reach, or
    // nothing. A streaming edge stops the search — never grow into the
    // unknown.
    let mut landing = None;
    for k in 1..=DRIP_REACH {
        let c = [pos[0], pos[1] - k, pos[2]];
        match get_block(c) {
            Some(b) if b == d.air => continue,
            Some(b) => {
                landing = Some((c, b, k));
                break;
            }
            None => return,
        }
    }
    let roll = rng_u64("dripstone");
    if let Some((c, b, _)) = landing {
        if let Some(filled) = d.vessel_fill(b) {
            if roll % 1000 < VESSEL_PER_MILLE {
                swap_block(c, filled);
            }
            return;
        }
    }
    // A drip fills a vessel whoever put the spike there: that changes the
    // player's pot, not the cave, so the branch above is ungated. GROWTH is
    // what must never touch a generated formation — the cultivated gate
    // sits HERE, and after the chance roll, so the KV read is paid only by
    // the few ticks that would otherwise grow something.
    if roll % 1000 >= GROW_PER_MILLE || !is_placed(pos) {
        return;
    }
    let can_extend = len < MAX_RUN && landing.is_none_or(|(_, _, k)| k > 1);
    let raise = landing.and_then(|(c, b, k)| stalagmite_target(d, c, b, k));
    let extend = match (can_extend, raise) {
        (true, Some(_)) => (roll >> 10) & 1 == 0,
        (true, None) => true,
        (false, _) => false,
    };
    // What grows out of a cultivated spike is cultivated too, or a farm
    // would stop dead after its first segment.
    if extend {
        // The new free end inherits the drip it grew from, so a farm keeps
        // showing that it is live without waiting for a random tick.
        grow_into(below, d.stalactite_wet);
    } else if let Some(target) = raise {
        grow_into(target, d.stalagmite);
    }
}

/// Write a grown spike and carry the cultivated mark onto it. The block
/// write clears the cell's KV, so the mark has to follow it, never precede.
fn grow_into(pos: [i32; 3], block: BlockId) {
    if set_block(pos, block) {
        mark_placed(pos);
    }
}

/// Remember that a player put this spike here (see [`PLACED_KEY`]).
pub fn on_placed(d: &Dripstone, payload: &EventPayload) -> Outcome {
    if let EventPayload::BlockPlaced { pos, block } = payload {
        // The event names the HELD row, which for a ceiling click is the
        // sibling of the row actually written — either way it is one of
        // ours, and the mark belongs to the CELL.
        if d.is_spike(*block) {
            mark_placed(*pos);
        }
    }
    Outcome::Continue
}

fn mark_placed(pos: [i32; 3]) {
    section_kv_set(pos, PLACED_KEY, vec![1]);
}

fn is_placed(pos: [i32; 3]) -> bool {
    section_kv_get(pos, PLACED_KEY).is_some()
}

/// Whether a run hung from `support` drips: a water source sits directly
/// over the block the run hangs from. ANY block — water seeps through a
/// ceiling, so what matters is that there is water up there.
///
/// It used to demand that the block be a dripstone block specifically (the
/// reference game's rule). Nothing in the world could tell a player that,
/// and it cost two playtests: a stone ceiling with water on top is the
/// obvious thing to build, it looks exactly like a working farm, and it did
/// nothing. A requirement a player cannot discover is not a rule, it is a
/// trap.
fn drips(d: &Dripstone, support: [i32; 3]) -> bool {
    get_block([support[0], support[1] + 1, support[2]]) == Some(d.water)
}

/// The cell a drip landing on `(c, b)` at distance `k` would raise a
/// stalagmite into: the air above an existing stalagmite tip whose run is
/// still short, or above any full surface. Nothing when the landing is
/// right under the tip (no room) — the two would already be touching.
fn stalagmite_target(d: &Dripstone, c: [i32; 3], b: BlockId, k: i32) -> Option<[i32; 3]> {
    if k < 2 {
        return None;
    }
    let above = [c[0], c[1] + 1, c[2]];
    if b == d.stalagmite {
        let (len, _) = d.run_root(c, Run::Standing);
        return (len < MAX_RUN).then_some(above);
    }
    matches!(collision_shape_at(c), Some(CollisionShape::Full)).then_some(above)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, OnceLock};

    const AIR: BlockId = BlockId(0);
    const STALACTITE: BlockId = BlockId(101);
    const STALACTITE_WET: BlockId = BlockId(103);
    const STALAGMITE: BlockId = BlockId(102);
    const ROCK: BlockId = BlockId(200);
    const DRIPSTONE: BlockId = BlockId(201);
    const WATER: BlockId = BlockId(202);
    const VESSEL: BlockId = BlockId(203);
    const FILLED: BlockId = BlockId(204);

    /// A synthetic world answering the host calls this behaviour makes, and
    /// modelling the ONE engine rule it rides: a block write announces to the
    /// written cell and its six neighbours, and those updates dispatch on the
    /// NEXT tick — the running tick's batch was snapshotted before the write
    /// (`World::game_tick` phase 2).
    #[derive(Default)]
    struct Fake {
        blocks: BTreeMap<[i32; 3], u16>,
        /// Per-cell KV, cleared by a block write exactly as the engine
        /// clears it — so a mark written BEFORE its block would be lost
        /// here too.
        kv: BTreeMap<([i32; 3], String), Vec<u8>>,
        queued: BTreeSet<[i32; 3]>,
        launched: Vec<[i32; 3]>,
        dropped: Vec<[i32; 3]>,
        /// Answers to `rng_u64`, consumed in order; empty answers 0.
        rolls: Vec<u64>,
        /// The loaded region: a cell inside it with no entry is AIR, one
        /// outside is UNLOADED (`get_block` answers `None`). Empty by
        /// default, so a test that wants a streaming edge simply says
        /// nothing.
        loaded: Option<([i32; 3], [i32; 3])>,
    }

    impl Fake {
        /// Scene setup: no announcement (the world was generated this way).
        fn place(&mut self, pos: [i32; 3], block: BlockId) {
            self.blocks.insert(pos, block.0);
        }

        /// An edit, announced exactly as the engine announces one — and
        /// clearing the cell's KV, which every block write does.
        fn write(&mut self, pos: [i32; 3], block: BlockId) {
            self.blocks.insert(pos, block.0);
            self.kv.retain(|(cell, _), _| *cell != pos);
            for d in [
                [0, 0, 0],
                [1, 0, 0],
                [-1, 0, 0],
                [0, 1, 0],
                [0, -1, 0],
                [0, 0, 1],
                [0, 0, -1],
            ] {
                self.queued
                    .insert([pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]]);
            }
        }

        /// The next tick's update batch, snapshotted and cleared.
        fn take_batch(&mut self) -> Vec<[i32; 3]> {
            std::mem::take(&mut self.queued).into_iter().collect()
        }

        fn block(&self, pos: [i32; 3]) -> Option<BlockId> {
            self.blocks.get(&pos).copied().map(BlockId)
        }

        /// Everything in this box is loaded; unset cells there read as air.
        fn load_box(&mut self, min: [i32; 3], max: [i32; 3]) {
            self.loaded = Some((min, max));
        }

        /// What a `get_block` sees: the written block, else air inside the
        /// loaded region, else nothing at all.
        fn seen(&self, pos: [i32; 3]) -> Option<BlockId> {
            if let Some(id) = self.blocks.get(&pos) {
                return Some(BlockId(*id));
            }
            let (min, max) = self.loaded?;
            (0..3)
                .all(|a| (min[a]..=max[a]).contains(&pos[a]))
                .then_some(AIR)
        }

        /// Mark a cell cultivated without going through a placement.
        fn mark(&mut self, pos: [i32; 3]) {
            self.kv.insert((pos, super::PLACED_KEY.to_owned()), vec![1]);
        }

        fn placed(&self, pos: [i32; 3]) -> bool {
            self.kv.contains_key(&(pos, super::PLACED_KEY.to_owned()))
        }

        fn spikes(&self) -> Vec<[i32; 3]> {
            let mut v: Vec<[i32; 3]> = self
                .blocks
                .iter()
                .filter(|(_, id)| {
                    **id == STALACTITE.0 || **id == STALACTITE_WET.0 || **id == STALAGMITE.0
                })
                .map(|(p, _)| *p)
                .collect();
            v.sort_by_key(|p| -p[1]);
            v
        }
    }

    static WORLD: Mutex<Option<Fake>> = Mutex::new(None);
    /// Serialises the tests that share the one world behind the process-wide
    /// native host; never held across a host call.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn with<R>(f: impl FnOnce(&mut Fake) -> R) -> R {
        let mut guard = WORLD.lock().unwrap_or_else(|e| e.into_inner());
        f(guard.get_or_insert_with(Fake::default))
    }

    fn cell(p: [f32; 3]) -> [i32; 3] {
        [
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        ]
    }

    /// Route the SDK's host calls at [`WORLD`] so the REAL behaviour code
    /// runs off-wasm. Any call this world does not model panics, so a future
    /// edit that reaches for one is reported rather than silently answered.
    fn install_host() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            let _ = mod_sdk::__rt::NATIVE_HOST.set(Box::new(|call| match call {
                HostCall::GetBlock { pos } => HostRet::Block(with(|f| f.seen(*pos))),
                HostCall::SetBlock { pos, block } => {
                    with(|f| f.write(*pos, *block));
                    HostRet::Bool(true)
                }
                HostCall::CollisionShapeAt { pos } => HostRet::CollisionShape(with(|f| {
                    f.blocks.get(pos).map(|id| {
                        if *id == ROCK.0 || *id == DRIPSTONE.0 {
                            CollisionShape::Full
                        } else {
                            CollisionShape::Empty
                        }
                    })
                })),
                HostCall::SwapBlock { pos, block } => {
                    with(|f| f.blocks.insert(*pos, block.0));
                    HostRet::Bool(true)
                }
                HostCall::SectionKvGet { pos, key } => {
                    HostRet::Bytes(with(|f| f.kv.get(&(*pos, key.clone())).cloned()))
                }
                HostCall::SectionKvSet { pos, key, value } => {
                    with(|f| f.kv.insert((*pos, key.clone()), value.clone()));
                    HostRet::Bool(true)
                }
                HostCall::RngU64 { .. } => HostRet::U64(with(|f| {
                    if f.rolls.is_empty() {
                        0
                    } else {
                        f.rolls.remove(0)
                    }
                })),
                HostCall::LaunchItem { pos, .. } => {
                    with(|f| f.launched.push(cell(*pos)));
                    HostRet::U64(1)
                }
                HostCall::SpawnItem { pos, .. } => {
                    with(|f| f.dropped.push(cell(*pos)));
                    HostRet::Bool(true)
                }
                other => panic!("the fake world does not model {other:?}"),
            }));
        });
    }

    /// A fresh world with the host installed.
    fn fake() -> Fake {
        install_host();
        Fake::default()
    }

    fn dripstone() -> Dripstone {
        Dripstone {
            block: DRIPSTONE,
            stalactite: STALACTITE,
            stalactite_wet: STALACTITE_WET,
            stalagmite: STALAGMITE,
            water: WATER,
            air: AIR,
            vessels: Vec::new(),
            biome: None,
        }
    }

    /// Dispatch the pending update batch as one tick would, and answer how
    /// many spike cells are left afterward.
    fn tick(d: &Dripstone) -> usize {
        for pos in with(|f| f.take_batch()) {
            on_hook(d, BlockHookKind::NeighborUpdate, pos);
        }
        with(|f| f.spikes().len())
    }

    /// A run that loses its support UNZIPS — one cell per tick, carried by
    /// the engine's block-update cascade — and never vanishes whole. The hook
    /// is handed one cell and must take only that one; what takes the next is
    /// the update its own removal announced. Clearing the run here instead
    /// would be a second way to sequence one break, and it would outrun the
    /// cascade: the whole run went in a single tick.
    #[test]
    fn a_falling_run_unzips_one_cell_per_tick() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        install_host();
        let d = dripstone();
        with(|f| {
            *f = Fake::default();
            f.place([0, 10, 0], ROCK);
            for y in 6..=9 {
                f.place([0, y, 0], STALACTITE);
            }
        });
        // Mine the ceiling the run hangs from; the engine announces that.
        with(|f| f.write([0, 10, 0], AIR));

        let left: Vec<usize> = (0..4).map(|_| tick(&d)).collect();
        assert_eq!(left, vec![3, 2, 1, 0], "one segment per tick, top down");
        // Every segment fell as a flying piece, from its own cell, in order.
        assert_eq!(
            with(|f| f.launched.clone()),
            vec![[0, 9, 0], [0, 8, 0], [0, 7, 0], [0, 6, 0]]
        );
        assert!(
            with(|f| f.dropped.is_empty()),
            "a stalactite falls, it does not crumble"
        );
    }

    /// The same rule mirrored: a standing run unzips upward from the cell
    /// that lost its floor, and crumbles instead of falling.
    #[test]
    fn a_standing_run_unzips_upward_and_crumbles() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        install_host();
        let d = dripstone();
        with(|f| {
            *f = Fake::default();
            f.place([0, 0, 0], ROCK);
            for y in 1..=3 {
                f.place([0, y, 0], STALAGMITE);
            }
        });
        with(|f| f.write([0, 0, 0], AIR));

        let left: Vec<usize> = (0..3).map(|_| tick(&d)).collect();
        assert_eq!(left, vec![2, 1, 0], "one segment per tick, bottom up");
        assert_eq!(
            with(|f| f.dropped.clone()),
            vec![[0, 1, 0], [0, 2, 0], [0, 3, 0]]
        );
        assert!(with(|f| f.launched.is_empty()));
    }

    /// A segment whose support is intact is left alone, and an unloaded
    /// rootward cell counts as support — a streaming edge must never bring a
    /// run down.
    #[test]
    fn a_held_segment_survives_its_neighbour_update() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        install_host();
        let d = dripstone();
        with(|f| {
            *f = Fake::default();
            f.place([0, 10, 0], ROCK);
            f.place([0, 9, 0], STALACTITE);
            f.place([0, 8, 0], STALACTITE);
            // A second run whose ceiling is not loaded at all.
            f.place([5, 9, 5], STALACTITE);
            f.queued.insert([0, 9, 0]);
            f.queued.insert([0, 8, 0]);
            f.queued.insert([5, 9, 5]);
        });
        assert_eq!(tick(&d), 3, "nothing unsupported, nothing breaks");
        assert!(with(|f| f.launched.is_empty() && f.dropped.is_empty()));
    }

    /// A roll that passes the growth gate and picks each branch: the gate is
    /// `roll % 1000 < GROW_PER_MILLE`, the branch is bit 10.
    const EXTEND: u64 = 100;
    const RAISE: u64 = 1124;
    /// Fails the gate outright.
    const IDLE: u64 = 500;

    /// A player's farm: water over a dripstone block over a hanging tip,
    /// with a stone floor five cells under the drip.
    fn farm(marked: bool) -> Dripstone {
        let d = dripstone();
        with(|f| {
            *f = fake();
            f.place([0, 12, 0], WATER);
            f.place([0, 11, 0], DRIPSTONE);
            f.place([0, 10, 0], STALACTITE);
            f.place([0, 5, 0], ROCK);
            f.load_box([-1, 4, -1], [1, 13, 1]);
            if marked {
                f.mark([0, 10, 0]);
            }
        });
        d
    }

    fn random_tick(d: &Dripstone, pos: [i32; 3], roll: u64) {
        with(|f| f.rolls.push(roll));
        on_hook(d, BlockHookKind::RandomTick, pos);
    }

    /// BOTH growth directions must be reachable from one dripping tip: the
    /// stalactite lengthens downward, and a stalagmite rises from the floor
    /// the drip lands on. Which one a tick takes is the coin flip in the
    /// roll, so a farm alternates and the two eventually meet.
    #[test]
    fn a_dripping_tip_both_grows_downward_and_raises_a_stalagmite() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = farm(true);
        random_tick(&d, [0, 10, 0], EXTEND);
        assert_eq!(
            with(|f| f.block([0, 9, 0])),
            Some(STALACTITE_WET),
            "the tip lengthened downward, still visibly dripping"
        );
        // The new tip is cultivated too, or the farm stops after one segment.
        assert!(with(|f| f.placed([0, 9, 0])), "growth carries the mark");

        // From the new tip, the other branch raises a stalagmite off the floor.
        random_tick(&d, [0, 9, 0], RAISE);
        assert_eq!(
            with(|f| f.block([0, 6, 0])),
            Some(STALAGMITE),
            "a stalagmite rose from the floor under the drip"
        );
        assert!(with(|f| f.placed([0, 6, 0])));

        // And it keeps rising on later drips, from its own tip upward.
        random_tick(&d, [0, 9, 0], RAISE);
        assert_eq!(with(|f| f.block([0, 7, 0])), Some(STALAGMITE));
    }

    /// A tick that fails the chance gate changes nothing — growth is rare,
    /// not guaranteed.
    #[test]
    fn an_idle_roll_grows_nothing() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = farm(true);
        random_tick(&d, [0, 10, 0], IDLE);
        assert_eq!(with(|f| f.block([0, 9, 0])), None);
    }

    /// THE CAVE KEEPS ITS SHAPE. A spike the world generated carries no
    /// cultivated mark, so however long it drips it never grows — only what
    /// a player placed does.
    #[test]
    fn a_naturally_generated_spike_never_grows() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = farm(false);
        for _ in 0..50 {
            random_tick(&d, [0, 10, 0], EXTEND);
            random_tick(&d, [0, 10, 0], RAISE);
        }
        assert_eq!(with(|f| f.spikes()), vec![[0, 10, 0]], "nothing grew");
    }

    /// A NATURAL spike's drip still fills a vessel under it. The cultivated
    /// gate exists to keep a generated cave's SHAPE, and a pot filling
    /// changes the player's pot, not the cave — so finding a wild drip and
    /// putting something under it has to work.
    #[test]
    fn a_natural_drip_still_fills_a_vessel() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = farm(false);
        d.vessels = vec![(VESSEL, FILLED)];
        with(|f| {
            f.place([0, 5, 0], VESSEL);
            f.rolls.push(EXTEND);
        });
        on_hook(&d, BlockHookKind::RandomTick, [0, 10, 0]);
        assert_eq!(
            with(|f| f.block([0, 5, 0])),
            Some(FILLED),
            "the wild drip filled the pot"
        );
        assert_eq!(with(|f| f.spikes()), vec![[0, 10, 0]], "and grew nothing");
    }

    /// Placing a spike is what marks it, and the mark dies with the block —
    /// so a cultivated spike that falls and regenerates comes back wild.
    #[test]
    fn placing_marks_the_cell_and_a_block_write_clears_it() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = dripstone();
        with(|f| *f = fake());
        on_placed(
            &d,
            &EventPayload::BlockPlaced {
                pos: [2, 3, 4],
                block: STALACTITE,
            },
        );
        assert!(with(|f| f.placed([2, 3, 4])));
        with(|f| f.write([2, 3, 4], AIR));
        assert!(
            !with(|f| f.placed([2, 3, 4])),
            "the mark dies with the block"
        );
    }

    /// THE DRIP MEANS WATER. A tip hanging under a dripstone block with a
    /// water source over it wears the emitter row; take the water away and it
    /// sheds it. Before this the emitter sat on every hanging row, so a spike
    /// hung from bare stone visibly dripped while the simulation saw no water
    /// at all — the feedback said "this farm is live" about a farm that could
    /// never grow (Rachel's playtest).
    #[test]
    fn a_tip_wears_the_drip_only_while_it_is_really_dripping() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = farm(true);
        // The farm is built dry-skinned; one tick of its own hook dresses it.
        on_hook(&d, BlockHookKind::NeighborUpdate, [0, 10, 0]);
        assert_eq!(
            with(|f| f.block([0, 10, 0])),
            Some(STALACTITE_WET),
            "water over a dripstone block means the tip drips"
        );
        // The cultivated mark must survive the swap, or the farm quietly
        // stops being a farm.
        assert!(with(|f| f.placed([0, 10, 0])), "the swap carried the mark");

        // Take the water away: the tip sheds the drip.
        with(|f| f.place([0, 12, 0], AIR));
        on_hook(&d, BlockHookKind::NeighborUpdate, [0, 10, 0]);
        assert_eq!(with(|f| f.block([0, 10, 0])), Some(STALACTITE));
    }

    /// What makes a tip drip is WATER over its run, whatever the ceiling is
    /// made of — a stone ceiling with water on top is the obvious thing to
    /// build and has to work.
    #[test]
    fn any_ceiling_with_water_over_it_drips() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = dripstone();
        with(|f| {
            *f = fake();
            f.load_box([-1, 0, -1], [1, 20, 1]);
            f.place([0, 12, 0], WATER);
            f.place([0, 11, 0], ROCK);
            f.place([0, 10, 0], STALACTITE);
        });
        on_hook(&d, BlockHookKind::NeighborUpdate, [0, 10, 0]);
        assert_eq!(with(|f| f.block([0, 10, 0])), Some(STALACTITE_WET));
    }

    /// A spike with nothing wet above it never drips, however long it hangs.
    #[test]
    fn a_spike_with_no_water_over_it_never_drips() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = dripstone();
        with(|f| {
            *f = fake();
            f.load_box([-1, 0, -1], [1, 20, 1]);
            f.place([0, 11, 0], ROCK);
            f.place([0, 10, 0], STALACTITE);
            f.mark([0, 10, 0]);
            f.rolls.push(EXTEND);
        });
        on_hook(&d, BlockHookKind::RandomTick, [0, 10, 0]);
        assert_eq!(with(|f| f.block([0, 10, 0])), Some(STALACTITE), "stays dry");
        assert_eq!(with(|f| f.block([0, 9, 0])), None, "and grows nothing");
    }

    /// Wet and dry are one RUN, not two: a wet tip under dry segments is
    /// held by them, and a run that loses its ceiling still unzips whole.
    #[test]
    fn a_wet_tip_belongs_to_the_run_above_it() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = dripstone();
        with(|f| {
            *f = fake();
            f.load_box([-1, 0, -1], [1, 20, 1]);
            f.place([0, 12, 0], ROCK);
            f.place([0, 11, 0], STALACTITE);
            f.place([0, 10, 0], STALACTITE);
            f.place([0, 9, 0], STALACTITE_WET);
        });
        // Nothing falls: the wet tip's support is the dry segment above it.
        for pos in [[0, 11, 0], [0, 10, 0], [0, 9, 0]] {
            on_hook(&d, BlockHookKind::NeighborUpdate, pos);
        }
        assert_eq!(with(|f| f.spikes().len()), 3, "a mixed run stands");

        // Cut the ceiling: the whole run unzips, wet segment included.
        with(|f| f.write([0, 12, 0], AIR));
        let left: Vec<usize> = (0..3).map(|_| tick(&d)).collect();
        assert_eq!(left, vec![2, 1, 0]);
    }
}
