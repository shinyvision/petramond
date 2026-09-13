//! Candidate roots and clearance are positional facts shared by every section.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use super::{batched, highest_floor, Candidate, ANCHOR_LATTICE, PROBE_PER_CANDIDATE};
use mod_sdk::{
    memo_get_many, memo_put, terrain_space_at, underground_biome_at, ByteReader, ByteWriter,
    TerrainSpace, SIM_BATCH_MAX,
};

type Root = [i32; 3];
type Key = (u32, u8, [i32; 3]);
const CAPACITY: usize = 8192;

#[derive(Default)]
struct Cache {
    decisions: HashMap<Key, Option<Root>>,
    order: VecDeque<Key>,
}

thread_local! {
    static ROOTS: RefCell<Cache> = RefCell::new(Cache::default());
}

/// One anchor cell's verdict in the shared memo (scoped by the host to this
/// mod and the world seed).
fn memo_key(ours: u8, lat: [i32; 3]) -> Vec<u8> {
    let mut w = ByteWriter::with_capacity(14);
    w.raw(&[b'g', ours]);
    w.i32x3(lat);
    w.finish()
}

fn encode_root(root: Option<Root>) -> Vec<u8> {
    let mut w = ByteWriter::with_capacity(13);
    match root {
        None => w.raw(&[0]),
        Some(p) => {
            w.raw(&[1]);
            w.i32x3(p);
        }
    }
    w.finish()
}

fn decode_root(bytes: &[u8]) -> Option<Option<Root>> {
    let mut r = ByteReader::new(bytes);
    match r.take(1)? {
        [0] => Some(None),
        [1] => Some(Some(r.i32x3()?)),
        _ => None,
    }
}

pub(super) fn viable_roots(
    seed: u32,
    ours: u8,
    candidates: &[Candidate],
) -> Option<Vec<(usize, Root)>> {
    let mut roots = vec![None; candidates.len()];
    let missing = ROOTS.with(|cache| {
        let cache = cache.borrow();
        let mut missing = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            match cache.decisions.get(&(seed, ours, candidate.lat)) {
                Some(root) => roots[index] = *root,
                None => missing.push(index),
            }
        }
        missing
    });
    if !missing.is_empty() {
        // Verdicts another worker already settled come from the shared memo;
        // only the rest are probed, and those are published for everyone.
        let mut resolved: Vec<(usize, Option<Root>)> = Vec::with_capacity(missing.len());
        let mut unresolved = Vec::new();
        for chunk in missing.chunks(SIM_BATCH_MAX) {
            let keys = chunk
                .iter()
                .map(|&i| memo_key(ours, candidates[i].lat))
                .collect();
            let shared = memo_get_many(keys);
            for (k, &i) in chunk.iter().enumerate() {
                match shared
                    .get(k)
                    .and_then(|b| b.as_deref())
                    .and_then(decode_root)
                {
                    Some(root) => resolved.push((i, root)),
                    None => unresolved.push(i),
                }
            }
        }
        if !unresolved.is_empty() {
            let batch: Vec<_> = unresolved.iter().map(|&i| &candidates[i]).collect();
            // A failed host reply is not a positional rejection and must not persist.
            let decisions = probe_roots(ours, &batch)?;
            for (&i, root) in unresolved.iter().zip(decisions) {
                memo_put(&memo_key(ours, candidates[i].lat), encode_root(root));
                resolved.push((i, root));
            }
        }
        ROOTS.with(|cache| {
            let mut cache = cache.borrow_mut();
            for (index, root) in resolved {
                let key = (seed, ours, candidates[index].lat);
                if cache.decisions.len() == CAPACITY {
                    let old = cache.order.pop_front().expect("full root cache");
                    cache.decisions.remove(&old);
                }
                cache.decisions.insert(key, root);
                cache.order.push_back(key);
                roots[index] = root;
            }
        });
    }
    Some(
        roots
            .into_iter()
            .enumerate()
            .filter_map(|(i, root)| root.map(|p| (i, p)))
            .collect(),
    )
}

fn probe_roots(ours: u8, cands: &[&Candidate]) -> Option<Vec<Option<Root>>> {
    // Biome gate at the middle of the root window, the same fixed point every
    // other pass asks about.
    let gate: Vec<[i32; 3]> = cands
        .iter()
        .map(|c| [c.x, c.cell_floor_y + ANCHOR_LATTICE / 2, c.z])
        .collect();
    let want = gate.len();
    let biomes = batched(gate, underground_biome_at);
    if biomes.len() != want {
        return None;
    }
    // Roots for the biome survivors: one column span each, one crossing.
    let mut probe: Vec<[i32; 3]> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        if biomes[i] != ours {
            continue;
        }
        spans.push((i, probe.len()));
        for wy in (c.cell_floor_y - 1)..=c.cell_top_y() {
            probe.push([c.x, wy, c.z]);
        }
    }
    let want = probe.len();
    let space = batched(probe, terrain_space_at);
    if space.len() != want {
        return None;
    }
    let mut rooted: Vec<(usize, [i32; 3])> = Vec::new();
    for &(i, start) in &spans {
        let c = &cands[i];
        if let Some(k) = highest_floor(&space[start..start + PROBE_PER_CANDIDATE]) {
            rooted.push((i, [c.x, c.cell_floor_y - 1 + k as i32, c.z]));
        }
    }
    // The fit skeletons, one more crossing for all of them together.
    let mut probe: Vec<[i32; 3]> = Vec::new();
    let mut fit_at: Vec<usize> = Vec::with_capacity(rooted.len());
    for &(i, root) in &rooted {
        fit_at.push(probe.len());
        cands[i].giant.fit_probes(|dx, dy, dz| {
            probe.push([root[0] + dx, root[1] + dy, root[2] + dz]);
        });
    }
    let want = probe.len();
    let space = batched(probe, terrain_space_at);
    if space.len() != want {
        return None;
    }
    let mut viable = vec![None; cands.len()];
    for (k, &(i, root)) in rooted.iter().enumerate() {
        let end = fit_at.get(k + 1).copied().unwrap_or(space.len());
        // Every cell the body fills must be free ROOM; a fluid is not room.
        if space[fit_at[k]..end]
            .iter()
            .all(|&s| s == TerrainSpace::Air)
        {
            viable[i] = Some(root);
        }
    }
    Some(viable)
}
