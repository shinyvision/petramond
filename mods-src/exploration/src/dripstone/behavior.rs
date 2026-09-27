use mod_sdk::*;

use super::{Dripstone, Run, MAX_RUN, PLACED_KEY};
use crate::keys;

const GROW_PER_MILLE: u64 = 150;
const VESSEL_PER_MILLE: u64 = 250;
const DRIP_REACH: i32 = 11;
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
        return None;
    }
    let (_, support) = d.run_root(pos, Run::Hanging);
    let wet = drips(d, support);
    let want = if wet { d.stalactite_wet } else { d.stalactite };
    if block != want {
        swap_block(pos, want);
    }
    Some(wet)
}

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
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 0.5,
        pos[2] as f64 + 0.5,
    ];
    if d.run_of(block) == Some(Run::Hanging) {
        launch_item(
            keys::POINTED_DRIPSTONE_ITEM,
            centre,
            [0.0, -FALL_SPEED, 0.0],
            None,
            &[],
        );
    } else {
        spawn_item(keys::POINTED_DRIPSTONE_ITEM, 1, centre);
    }
}

fn grow(d: &Dripstone, pos: [i32; 3], block: BlockId) {
    if wetness(d, pos, block) != Some(true) {
        return;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    let (len, _) = d.run_root(pos, Run::Hanging);
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
    if extend {
        grow_into(below, d.stalactite_wet);
    } else if let Some(target) = raise {
        grow_into(target, d.stalagmite);
    }
}

fn grow_into(pos: [i32; 3], block: BlockId) {
    if set_block(pos, block) {
        mark_placed(pos);
    }
}

pub fn on_placed(d: &Dripstone, payload: &EventPayload) -> Outcome {
    if let EventPayload::BlockPlaced { pos, block } = payload {
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

fn drips(d: &Dripstone, support: [i32; 3]) -> bool {
    get_block([support[0], support[1] + 1, support[2]]) == Some(d.water)
}

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
    use std::sync::Mutex;

    const AIR: BlockId = BlockId::AIR;
    const STALACTITE: BlockId = BlockId(101);
    const STALACTITE_WET: BlockId = BlockId(103);
    const STALAGMITE: BlockId = BlockId(102);
    const ROCK: BlockId = BlockId(200);
    const DRIPSTONE: BlockId = BlockId(201);
    const WATER: BlockId = BlockId(202);
    const VESSEL: BlockId = BlockId(203);
    const LAVA: BlockId = BlockId(205);
    const FILLED: BlockId = BlockId(204);

    /// Fake world for this behavior's host calls. The one engine rule it copies: writing a block
    /// pings that cell and its six neighbors on the next tick, because the current tick's batch
    /// was snapshotted before the write (`World::game_tick` phase 2).
    #[derive(Default)]
    struct Fake {
        blocks: BTreeMap<[i32; 3], u16>,
        kv: BTreeMap<([i32; 3], String), Vec<u8>>,
        queued: BTreeSet<[i32; 3]>,
        launched: Vec<[i32; 3]>,
        dropped: Vec<[i32; 3]>,
        rolls: Vec<u64>,
        loaded: Option<([i32; 3], [i32; 3])>,
    }

    impl Fake {
        fn place(&mut self, pos: [i32; 3], block: BlockId) {
            self.blocks.insert(pos, block.0);
        }

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

        fn take_batch(&mut self) -> Vec<[i32; 3]> {
            std::mem::take(&mut self.queued).into_iter().collect()
        }

        fn block(&self, pos: [i32; 3]) -> Option<BlockId> {
            self.blocks.get(&pos).copied().map(BlockId)
        }

        fn load_box(&mut self, min: [i32; 3], max: [i32; 3]) {
            self.loaded = Some((min, max));
        }

        fn seen(&self, pos: [i32; 3]) -> Option<BlockId> {
            if let Some(id) = self.blocks.get(&pos) {
                return Some(BlockId(*id));
            }
            let (min, max) = self.loaded?;
            (0..3)
                .all(|a| (min[a]..=max[a]).contains(&pos[a]))
                .then_some(AIR)
        }

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
    static SERIAL: Mutex<()> = Mutex::new(());

    fn with<R>(f: impl FnOnce(&mut Fake) -> R) -> R {
        let mut guard = WORLD.lock().unwrap_or_else(|e| e.into_inner());
        f(guard.get_or_insert_with(Fake::default))
    }

    fn cell(p: [f64; 3]) -> [i32; 3] {
        [
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        ]
    }

    fn install_host() {
        thread_local! {
            static HOST: std::cell::RefCell<Option<mod_sdk::testing::HostGuard>> =
                const { std::cell::RefCell::new(None) };
        }
        HOST.with(|slot| {
            slot.borrow_mut().get_or_insert_with(|| {
                mod_sdk::testing::install_host(|call| match call {
                    HostCall::Block(mod_sdk::BlockCall::GetBlock { pos }) => {
                        HostRet::Block(with(|f| f.seen(*pos)))
                    }
                    HostCall::Block(mod_sdk::BlockCall::SetBlock { pos, block }) => {
                        with(|f| f.write(*pos, *block));
                        HostRet::Bool(true)
                    }
                    HostCall::Block(mod_sdk::BlockCall::CollisionShapeAt { pos }) => {
                        HostRet::CollisionShape(with(|f| {
                            f.blocks.get(pos).map(|id| {
                                if *id == ROCK.0 || *id == DRIPSTONE.0 {
                                    CollisionShape::Full
                                } else {
                                    CollisionShape::Empty
                                }
                            })
                        }))
                    }
                    HostCall::Block(mod_sdk::BlockCall::SwapBlock { pos, block }) => {
                        with(|f| f.blocks.insert(*pos, block.0));
                        HostRet::Bool(true)
                    }
                    HostCall::Kv(mod_sdk::KvCall::SectionKvGet { pos, key }) => {
                        HostRet::Bytes(with(|f| f.kv.get(&(*pos, key.clone())).cloned()))
                    }
                    HostCall::Kv(mod_sdk::KvCall::SectionKvSet { pos, key, value }) => {
                        with(|f| f.kv.insert((*pos, key.clone()), value.clone()));
                        HostRet::Bool(true)
                    }
                    HostCall::Core(mod_sdk::CoreCall::RngU64 { .. }) => HostRet::U64(with(|f| {
                        if f.rolls.is_empty() {
                            0
                        } else {
                            f.rolls.remove(0)
                        }
                    })),
                    HostCall::Entity(mod_sdk::EntityCall::LaunchItem { pos, .. }) => {
                        with(|f| f.launched.push(cell(*pos)));
                        HostRet::U64(1)
                    }
                    HostCall::Entity(mod_sdk::EntityCall::SpawnItem { pos, .. }) => {
                        with(|f| f.dropped.push(cell(*pos)));
                        HostRet::Bool(true)
                    }
                    other => panic!("the fake world does not model {other:?}"),
                })
            });
        });
    }

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
            fluids: crate::fluids::Fluids::of(&[WATER, LAVA]),
            air: AIR,
            vessels: Vec::new(),
            biome: None,
        }
    }

    fn tick(d: &Dripstone) -> usize {
        for pos in with(|f| f.take_batch()) {
            on_hook(d, BlockHookKind::NeighborUpdate, pos);
        }
        with(|f| f.spikes().len())
    }

    /// An unsupported run unzips one cell per tick via the engine's block-update cascade; it never
    /// vanishes whole.
    /// The hook only takes the cell it's handed; the next cell goes when that removal fires an
    /// update.
    /// Clearing the whole run here would be a second way to sequence one break, and it would
    /// outrun the cascade.
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
        with(|f| f.write([0, 10, 0], AIR));

        let left: Vec<usize> = (0..4).map(|_| tick(&d)).collect();
        assert_eq!(left, vec![3, 2, 1, 0], "one segment per tick, top down");
        assert_eq!(
            with(|f| f.launched.clone()),
            vec![[0, 9, 0], [0, 8, 0], [0, 7, 0], [0, 6, 0]]
        );
        assert!(
            with(|f| f.dropped.is_empty()),
            "a stalactite falls, it does not crumble"
        );
    }

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
            f.place([5, 9, 5], STALACTITE);
            f.queued.insert([0, 9, 0]);
            f.queued.insert([0, 8, 0]);
            f.queued.insert([5, 9, 5]);
        });
        assert_eq!(tick(&d), 3, "nothing unsupported, nothing breaks");
        assert!(with(|f| f.launched.is_empty() && f.dropped.is_empty()));
    }

    const EXTEND: u64 = 100;
    const RAISE: u64 = 1124;
    const IDLE: u64 = 500;

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
        assert!(with(|f| f.placed([0, 9, 0])), "growth carries the mark");

        random_tick(&d, [0, 9, 0], RAISE);
        assert_eq!(
            with(|f| f.block([0, 6, 0])),
            Some(STALAGMITE),
            "a stalagmite rose from the floor under the drip"
        );
        assert!(with(|f| f.placed([0, 6, 0])));

        random_tick(&d, [0, 9, 0], RAISE);
        assert_eq!(with(|f| f.block([0, 7, 0])), Some(STALAGMITE));
    }

    #[test]
    fn an_idle_roll_grows_nothing() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = farm(true);
        random_tick(&d, [0, 10, 0], IDLE);
        assert_eq!(with(|f| f.block([0, 9, 0])), None);
    }

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

    #[test]
    fn a_tip_wears_the_drip_only_while_it_is_really_dripping() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = farm(true);
        on_hook(&d, BlockHookKind::NeighborUpdate, [0, 10, 0]);
        assert_eq!(
            with(|f| f.block([0, 10, 0])),
            Some(STALACTITE_WET),
            "water over a dripstone block means the tip drips"
        );
        assert!(with(|f| f.placed([0, 10, 0])), "the swap carried the mark");

        with(|f| f.place([0, 12, 0], AIR));
        on_hook(&d, BlockHookKind::NeighborUpdate, [0, 10, 0]);
        assert_eq!(with(|f| f.block([0, 10, 0])), Some(STALACTITE));
    }

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
        for pos in [[0, 11, 0], [0, 10, 0], [0, 9, 0]] {
            on_hook(&d, BlockHookKind::NeighborUpdate, pos);
        }
        assert_eq!(with(|f| f.spikes().len()), 3, "a mixed run stands");

        with(|f| f.write([0, 12, 0], AIR));
        let left: Vec<usize> = (0..3).map(|_| tick(&d)).collect();
        assert_eq!(left, vec![2, 1, 0]);
    }
}
