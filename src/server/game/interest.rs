//! Per-connection entity interest: which mobs, dropped items and players each
//! recipient tracks, so a tick window replicates only what that client can
//! see instead of the whole loaded population to everybody.
//!
//! An entity ENTERS a recipient's interest within its tracking radius
//! (horizontal distance from the player — the lane's own range, never beyond
//! the recipient's view distance) and LEAVES only past that radius plus
//! [`HYSTERESIS_BLOCKS`], or when it is gone from the world, so an entity
//! pacing along the edge does not flap between spawn and despawn. Entrants
//! are found through a per-window chunk-bucketed index, so a recipient's
//! refresh costs its neighbourhood plus what it already tracks — not the
//! world's entity count.
//!
//! The interest set is also where per-entity replication state for a
//! connection belongs (the baseline a delta-encoded update is diffed
//! against): its membership is exactly the recipient's spawn-to-despawn span.

use std::hash::Hash;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::player::PlayerId;
use petramond_math::world_pos::WorldPos;

/// How far mobs are tracked, in blocks (horizontal). Capped by the
/// recipient's view distance.
pub const MOB_TRACKING_BLOCKS: f64 = 160.0;

/// How far dropped items are tracked, in blocks (horizontal) — small bodies,
/// unreadable long before the mob range. Capped by the view distance.
pub const ITEM_TRACKING_BLOCKS: f64 = 96.0;

/// How far past its entry radius a tracked entity stays tracked.
pub const HYSTERESIS_BLOCKS: f64 = 16.0;

/// The narrowest view distance interest ever assumes, in chunks — the floor
/// the per-connection view request is clamped to.
pub const MIN_VIEW_CHUNKS: i32 = 4;

/// How far a recipient's view reaches, in blocks: its requested view distance
/// clamped to what the server streams at all (`server_cap`, chunks) and to
/// the [`MIN_VIEW_CHUNKS`] floor.
pub(super) fn view_blocks(requested_chunks: i32, server_cap: i32) -> f64 {
    let cap = server_cap.max(MIN_VIEW_CHUNKS);
    f64::from(requested_chunks.min(cap).max(MIN_VIEW_CHUNKS)) * 16.0
}

/// Chunk-column edge length in blocks: the index's bucket size.
const BUCKET_BLOCKS: f64 = 16.0;

/// One entity lane's positions for one tick window, bucketed by chunk column.
/// Entry `i` is the lane's `i`-th entity in world order — the same order the
/// replication tables are built in.
pub(super) struct LaneIndex<K> {
    keys: Vec<K>,
    xz: Vec<[f64; 2]>,
    buckets: FxHashMap<(i32, i32), Vec<u32>>,
    by_key: FxHashMap<K, u32>,
}

impl<K: Copy + Eq + Hash> LaneIndex<K> {
    pub(super) fn new(entities: impl Iterator<Item = (K, WorldPos)>) -> Self {
        let mut index = LaneIndex {
            keys: Vec::new(),
            xz: Vec::new(),
            buckets: FxHashMap::default(),
            by_key: FxHashMap::default(),
        };
        for (i, (key, pos)) in entities.enumerate() {
            let i = i as u32;
            index.keys.push(key);
            index.xz.push([pos.x, pos.z]);
            index
                .buckets
                .entry(bucket(pos.x, pos.z))
                .or_default()
                .push(i);
            index.by_key.insert(key, i);
        }
        index
    }

    /// The index of the entity keyed `key`, if it exists this window.
    pub(super) fn position_of(&self, key: K) -> Option<u32> {
        self.by_key.get(&key).copied()
    }

    fn distance_sq(&self, i: u32, view: Viewpoint) -> f64 {
        let [x, z] = self.xz[i as usize];
        (x - view.x).powi(2) + (z - view.z).powi(2)
    }

    /// Every entity within `radius` of `view` (and a few just outside it: the
    /// caller range-checks).
    fn for_each_near(&self, view: Viewpoint, radius: f64, mut f: impl FnMut(u32)) {
        let (x0, z0) = bucket(view.x - radius, view.z - radius);
        let (x1, z1) = bucket(view.x + radius, view.z + radius);
        // A sparse lane is cheaper to scan than its bounding square of
        // buckets is to probe.
        let probes = (i64::from(x1) - i64::from(x0) + 1) * (i64::from(z1) - i64::from(z0) + 1);
        if probes as usize >= self.keys.len() {
            (0..self.keys.len() as u32).for_each(f);
            return;
        }
        for bx in x0..=x1 {
            for bz in z0..=z1 {
                if let Some(members) = self.buckets.get(&(bx, bz)) {
                    members.iter().copied().for_each(&mut f);
                }
            }
        }
    }
}

fn bucket(x: f64, z: f64) -> (i32, i32) {
    (
        (x / BUCKET_BLOCKS).floor() as i32,
        (z / BUCKET_BLOCKS).floor() as i32,
    )
}

/// Where a recipient stands and how far one lane reaches from there.
#[derive(Copy, Clone, Debug)]
pub(super) struct Viewpoint {
    x: f64,
    z: f64,
    radius: f64,
}

impl Viewpoint {
    pub(super) fn new(at: WorldPos, radius: f64) -> Self {
        Viewpoint {
            x: at.x,
            z: at.z,
            radius,
        }
    }
}

/// One lane's interest change for one recipient: indices into the window's
/// [`LaneIndex`] (ascending) plus the ids that left (ascending).
#[derive(Debug, Default, PartialEq)]
pub(super) struct LaneSelection<K> {
    pub spawned: Vec<u32>,
    pub updated: Vec<u32>,
    pub despawned: Vec<K>,
}

impl<K> LaneSelection<K> {
    /// Every tracked index this window (spawns and updates).
    pub(super) fn tracked(&self) -> impl Iterator<Item = u32> + '_ {
        self.spawned.iter().chain(&self.updated).copied()
    }
}

/// The ids one recipient tracks in one entity lane.
pub struct InterestSet<K> {
    tracked: FxHashSet<K>,
}

impl<K> Default for InterestSet<K> {
    fn default() -> Self {
        InterestSet {
            tracked: FxHashSet::default(),
        }
    }
}

impl<K: Copy + Eq + Hash + Ord> InterestSet<K> {
    /// Advance to this window: keep what is still within the hysteresis
    /// radius, drop what left it or the world, admit what came within the
    /// entry radius. `forced` indices are tracked regardless of distance
    /// (the recipient itself, the mount under a tracked rider).
    pub(super) fn refresh(
        &mut self,
        index: &LaneIndex<K>,
        view: Viewpoint,
        forced: &[u32],
    ) -> LaneSelection<K> {
        let enter_sq = view.radius * view.radius;
        let stay_sq = (view.radius + HYSTERESIS_BLOCKS).powi(2);
        let mut next = FxHashSet::default();
        let mut sel = LaneSelection {
            spawned: Vec::new(),
            updated: Vec::new(),
            despawned: Vec::new(),
        };
        for &key in &self.tracked {
            match index.position_of(key) {
                Some(i) if index.distance_sq(i, view) <= stay_sq || forced.contains(&i) => {
                    sel.updated.push(i);
                    next.insert(key);
                }
                _ => sel.despawned.push(key),
            }
        }
        index.for_each_near(view, view.radius, |i| {
            if index.distance_sq(i, view) <= enter_sq && next.insert(index.keys[i as usize]) {
                sel.spawned.push(i);
            }
        });
        for &i in forced {
            if next.insert(index.keys[i as usize]) {
                sel.spawned.push(i);
            }
        }
        sel.spawned.sort_unstable();
        sel.updated.sort_unstable();
        sel.despawned.sort_unstable();
        self.tracked = next;
        sel
    }

    /// Whether this set tracks `key` as of its latest refresh.
    pub(super) fn contains(&self, key: K) -> bool {
        self.tracked.contains(&key)
    }
}

/// One connection's entity interest across the replicated lanes, plus the
/// looping sounds it currently hears (see `super::event_scope`).
#[derive(Default)]
pub struct EntityInterest {
    pub(super) mobs: InterestSet<u64>,
    pub(super) items: InterestSet<u64>,
    pub(super) players: InterestSet<PlayerId>,
    pub(super) loops: FxHashSet<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(entities: &[(u64, f64, f64)]) -> LaneIndex<u64> {
        LaneIndex::new(
            entities
                .iter()
                .map(|&(id, x, z)| (id, WorldPos::new(x, 64.0, z))),
        )
    }

    fn view(x: f64, radius: f64) -> Viewpoint {
        Viewpoint::new(WorldPos::new(x, 64.0, 0.0), radius)
    }

    #[test]
    fn entities_enter_within_the_radius_and_leave_only_past_the_hysteresis() {
        let mut set = InterestSet::default();
        let near = index(&[(1, 10.0, 0.0), (2, 200.0, 0.0)]);
        let sel = set.refresh(&near, view(0.0, 64.0), &[]);
        assert_eq!(sel.spawned, vec![0], "only the near entity enters");
        assert!(sel.updated.is_empty() && sel.despawned.is_empty());

        // Just outside the entry radius but inside the hysteresis band: kept.
        let band = index(&[(1, 64.0 + HYSTERESIS_BLOCKS - 1.0, 0.0)]);
        let sel = set.refresh(&band, view(0.0, 64.0), &[]);
        assert_eq!(sel.updated, vec![0], "a tracked entity rides the band");
        assert!(sel.spawned.is_empty() && sel.despawned.is_empty());

        // An untracked entity at the same spot does NOT enter.
        let mut fresh = InterestSet::default();
        assert!(fresh
            .refresh(&band, view(0.0, 64.0), &[])
            .spawned
            .is_empty());

        let gone = index(&[(1, 64.0 + HYSTERESIS_BLOCKS + 1.0, 0.0)]);
        let sel = set.refresh(&gone, view(0.0, 64.0), &[]);
        assert_eq!(sel.despawned, vec![1], "past the band it leaves");
        assert!(!set.contains(1));
    }

    #[test]
    fn a_removed_entity_despawns_and_a_returning_one_spawns_again() {
        let mut set = InterestSet::default();
        set.refresh(
            &index(&[(1, 0.0, 0.0), (2, 5.0, 0.0)]),
            view(0.0, 64.0),
            &[],
        );
        let sel = set.refresh(&index(&[(2, 5.0, 0.0)]), view(0.0, 64.0), &[]);
        assert_eq!(sel.despawned, vec![1], "gone from the world: despawned");
        assert_eq!(sel.updated, vec![0]);

        set.refresh(&index(&[(2, 500.0, 0.0)]), view(0.0, 64.0), &[]);
        let sel = set.refresh(&index(&[(2, 5.0, 0.0)]), view(0.0, 64.0), &[]);
        assert_eq!(sel.spawned, vec![0], "re-entry is a fresh spawn");
    }

    #[test]
    fn forced_entities_are_tracked_at_any_distance() {
        let mut set = InterestSet::default();
        let far = index(&[(1, 5000.0, 0.0)]);
        assert_eq!(set.refresh(&far, view(0.0, 64.0), &[0]).spawned, vec![0]);
        assert_eq!(set.refresh(&far, view(0.0, 64.0), &[0]).updated, vec![0]);
        assert_eq!(set.refresh(&far, view(0.0, 64.0), &[]).despawned, vec![1]);
    }

    /// The bucketed probe and the sparse-lane scan admit the same entities,
    /// across bucket edges and negative coordinates.
    #[test]
    fn the_bucketed_probe_finds_exactly_the_entities_in_range() {
        let entities: Vec<(u64, f64, f64)> = (0..400)
            .map(|i| {
                let f = f64::from(i);
                (
                    i as u64,
                    (f * 37.0) % 600.0 - 300.0,
                    (f * 53.0) % 600.0 - 300.0,
                )
            })
            .collect();
        let dense = index(&entities);
        let at = view(-17.5, 40.0);
        let mut set = InterestSet::default();
        let got = set.refresh(&dense, at, &[]).spawned;
        let want: Vec<u32> = entities
            .iter()
            .enumerate()
            .filter(|(_, &(_, x, z))| (x + 17.5).powi(2) + z * z <= 40.0 * 40.0)
            .map(|(i, _)| i as u32)
            .collect();
        assert!(!want.is_empty());
        assert_eq!(got, want);
    }
}
