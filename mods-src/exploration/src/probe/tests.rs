use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::*;

thread_local! {
    static CALLS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static SHORT: Cell<bool> = const { Cell::new(false) };
    static STORE: RefCell<HashMap<Vec<u8>, Vec<u8>>> = RefCell::new(HashMap::new());
    static CLAIM: RefCell<Option<MemoClaim>> = const { RefCell::new(None) };
    static REFUSE: Cell<bool> = const { Cell::new(false) };
}

fn reset() {
    CALLS.with(|c| c.borrow_mut().clear());
    SHORT.with(|s| s.set(false));
    STORE.with(|s| s.borrow_mut().clear());
    CLAIM.with(|c| *c.borrow_mut() = None);
    REFUSE.with(|r| r.set(false));
}

fn calls() -> Vec<usize> {
    CALLS.with(|c| c.borrow().clone())
}

fn classify(p: [i32; 3]) -> TerrainSpace {
    if p[1] < 0 {
        TerrainSpace::Solid
    } else if p[0] == 5 && p[1] == 0 {
        TerrainSpace::Fluid
    } else {
        TerrainSpace::Air
    }
}

fn fake_terrain(positions: Vec<[i32; 3]>) -> Vec<TerrainSpace> {
    CALLS.with(|c| c.borrow_mut().push(positions.len()));
    let mut out: Vec<TerrainSpace> = positions.into_iter().map(classify).collect();
    if SHORT.with(|s| s.get()) {
        out.pop();
    }
    out
}

fn fake_biomes_in_box(_lo: [i32; 3], hi: [i32; 3]) -> Vec<u8> {
    if hi[0] >= 100 {
        vec![1, 7]
    } else {
        vec![1]
    }
}

fn fake_claim(key: &[u8]) -> MemoClaim {
    if let Some(c) = CLAIM.with(|c| c.borrow().clone()) {
        return c;
    }
    match STORE.with(|s| s.borrow().get(key).cloned()) {
        Some(v) => MemoClaim::Value(v),
        None => MemoClaim::Lease,
    }
}

fn fake_put(key: &[u8], value: Vec<u8>) -> bool {
    if REFUSE.with(|r| r.get()) {
        return false;
    }
    STORE.with(|s| s.borrow_mut().insert(key.to_vec(), value));
    true
}

fn fake_get_many(keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>> {
    CALLS.with(|c| c.borrow_mut().push(keys.len()));
    let n = keys.len().saturating_sub(1);
    keys.into_iter()
        .take(n)
        .map(|k| STORE.with(|s| s.borrow().get(&k).cloned()))
        .collect()
}

const FAKE_MEMO: Memo = Memo {
    claim: fake_claim,
    put: fake_put,
};

#[test]
fn a_large_query_is_split_at_the_abi_cap_not_truncated() {
    reset();
    let n = SIM_BATCH_MAX * 2 + 3;
    let cells: Vec<[i32; 3]> = (0..n as i32).map(|i| [i, -1, 0]).collect();
    let reply = ask(cells, fake_terrain).expect("a full reply");
    assert_eq!(reply.len(), n);
    assert_eq!(calls(), vec![SIM_BATCH_MAX, SIM_BATCH_MAX, 3]);
    assert!(ask(Vec::new(), fake_terrain).is_some_and(|r| r.is_empty()));
    assert_eq!(calls().len(), 3, "an empty query must not cross the ABI");
}

#[test]
fn a_short_reply_is_never_an_answer() {
    reset();
    SHORT.with(|s| s.set(true));
    assert!(ask(vec![[0, 0, 0], [1, 0, 0]], fake_terrain).is_none());
}

#[test]
fn reads_are_keyed_by_position_across_asks() {
    reset();
    let mut reads = TerrainReads::with_query(fake_terrain);
    assert!(reads.ask([[0, -1, 0], [0, 3, 0], [0, -1, 0]]));
    assert_eq!(calls(), vec![2], "duplicates are asked once");
    assert!(reads.ask([[0, 3, 0], [5, 0, 0]]));
    assert_eq!(calls(), vec![2, 1], "answered cells are not asked again");
    assert!(reads.ask([[0, 3, 0]]));
    assert_eq!(calls().len(), 2, "a fully answered ask costs no crossing");

    assert_eq!(reads.solid([0, -1, 0]), Some(true));
    assert!(reads.rock([0, -1, 0]) && !reads.free([0, -1, 0]));
    assert!(reads.free([0, 3, 0]) && reads.solid([0, 3, 0]) == Some(false));
    assert!(!reads.rock([5, 0, 0]) && !reads.free([5, 0, 0]));
    assert_eq!(reads.solid([5, 0, 0]), Some(false));
    assert_eq!(reads.space([5, 0, 0]), Some(TerrainSpace::Fluid));
    assert_eq!(reads.space([9, 9, 9]), None);
    assert!(!reads.rock([9, 9, 9]) && !reads.free([9, 9, 9]));
}

#[test]
fn a_refused_batch_leaves_every_cell_unknown() {
    reset();
    let mut reads = TerrainReads::with_query(fake_terrain);
    assert!(reads.ask([[0, -1, 0]]));
    SHORT.with(|s| s.set(true));
    assert!(!reads.ask([[1, -1, 0], [2, -1, 0]]));
    assert_eq!(reads.solid([0, -1, 0]), None, "an earlier answer survived");
    assert_eq!(reads.solid([1, -1, 0]), None);
    SHORT.with(|s| s.set(false));
    assert!(!reads.ask([[3, -1, 0]]), "a failed read set stays failed");
}

#[test]
fn only_cells_outside_the_section_are_probed() {
    reset();
    let ctx = GenCtx::for_test(
        [0, 0, 0],
        1,
        vec![0u16; 4096],
        vec![64; 256],
        vec![0; 256],
        62,
    );
    let mut reads = TerrainReads::with_query(fake_terrain);
    assert!(reads.ask_unseen(&ctx, [[3, 3, 3], [3, -1, 3], [3, 16, 3]]));
    assert_eq!(calls(), vec![2], "the owned cell cost a probe");
    assert_eq!(reads.space([3, 3, 3]), None);
    assert!(reads.rock([3, -1, 3]) && reads.free([3, 16, 3]));
}

#[test]
fn a_pad_spans_the_section_plus_its_reach() {
    let pad = Pad {
        xz: 2,
        down: 5,
        up: 7,
    };
    assert_eq!(pad.around([16, -32, 0]), ([14, -37, -2], [33, -10, 17]));
    assert!(in_reach(
        7,
        [80, 0, 0],
        Pad {
            xz: 5,
            down: 0,
            up: 0
        },
        fake_biomes_in_box
    ));
    assert!(!in_reach(
        7,
        [80, 0, 0],
        Pad {
            xz: 0,
            down: 0,
            up: 0
        },
        fake_biomes_in_box
    ));
}

#[test]
fn settle_derives_once_publishes_and_defers_behind_a_lease() {
    reset();
    let derived = Cell::new(0);
    let run = || {
        settle(
            FAKE_MEMO,
            b"k",
            |b| (b.len() == 1).then(|| b[0]),
            |v| vec![*v],
            || {
                derived.set(derived.get() + 1);
                42u8
            },
        )
    };
    assert_eq!(run(), Ok(42));
    assert_eq!(derived.get(), 1);
    assert_eq!(run(), Ok(42), "the published value is reused");
    assert_eq!(derived.get(), 1, "a published fact was derived again");

    STORE.with(|s| s.borrow_mut().insert(b"k".to_vec(), vec![1, 2, 3]));
    assert_eq!(run(), Ok(42));
    assert_eq!(derived.get(), 2);
    assert_eq!(STORE.with(|s| s.borrow()[&b"k".to_vec()].clone()), vec![42]);

    CLAIM.with(|c| *c.borrow_mut() = Some(MemoClaim::Pending));
    assert_eq!(run(), Err(Deferred));
    assert_eq!(
        derived.get(),
        2,
        "a deferred section derived the fact anyway"
    );
}

#[test]
fn a_refused_publication_still_answers_and_is_derived_again_later() {
    reset();
    REFUSE.with(|r| r.set(true));
    let derived = Cell::new(0);
    let run = || {
        settle(
            FAKE_MEMO,
            b"big",
            |b| b.first().copied(),
            |v| vec![*v],
            || {
                derived.set(derived.get() + 1);
                9u8
            },
        )
    };
    assert_eq!(run(), Ok(9), "the lease holder keeps its own answer");
    assert!(STORE.with(|s| s.borrow().is_empty()), "nothing was stored");
    assert_eq!(run(), Ok(9));
    assert_eq!(derived.get(), 2, "an unpublished fact serves no one else");
}

#[test]
fn lookup_many_is_parallel_to_its_keys_even_when_the_host_answers_short() {
    reset();
    fake_put(b"a", vec![1]);
    let keys: Vec<Vec<u8>> = (0..SIM_BATCH_MAX + 2)
        .map(|i| {
            if i == 0 {
                b"a".to_vec()
            } else {
                vec![b'x', (i % 251) as u8]
            }
        })
        .collect();
    let got = lookup_many(fake_get_many, keys);
    assert_eq!(got.len(), SIM_BATCH_MAX + 2);
    assert_eq!(got[0], Some(vec![1]));
    assert!(got[1..].iter().all(Option::is_none));
    assert_eq!(calls(), vec![SIM_BATCH_MAX, 2]);
}

#[test]
fn the_settled_cache_evicts_oldest_first_and_updates_in_place() {
    let mut cache: Settled<u32, &str> = Settled::new(2);
    cache.insert(1, "a");
    cache.insert(2, "b");
    cache.insert(1, "a2");
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.get(&1), Some("a2"));
    cache.insert(3, "c");
    assert_eq!(cache.get(&1), None, "the oldest key survived eviction");
    assert_eq!(cache.get(&2), Some("b"));
    assert_eq!(cache.get(&3), Some("c"));
}
