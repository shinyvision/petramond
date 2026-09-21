//! A persistent set of `u64` ids in world KV, sharded so one change writes
//! one small value.

use std::collections::BTreeSet;

use crate::{world_kv_delete, world_kv_get, world_kv_set, KV_MAX_VALUE_BYTES};

/// Ids per shard by id RANGE: id `n` lives in shard `n / IDS_PER_SHARD`, so
/// adding or removing one id rewrites that shard alone, however large the
/// set. Suits ids handed out by a counter (dense, ever growing).
const IDS_PER_SHARD: u64 = 4096;
const _: () = assert!(IDS_PER_SHARD as usize * 8 <= KV_MAX_VALUE_BYTES);

/// The set, loaded once per session. World KV keys are `"{prefix}/{shard}"`
/// plus `"{prefix}/shards"` (how many shards to read back); the prefix must
/// sit in the mod's own namespace.
pub struct IdShards {
    prefix: String,
    ids: BTreeSet<u64>,
    /// Shards `0..shards` may hold a value.
    shards: u64,
}

impl IdShards {
    pub fn load(prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        let shards = world_kv_get(&format!("{prefix}/shards"))
            .and_then(|b| Some(u64::from_le_bytes(b.try_into().ok()?)))
            .unwrap_or(0);
        let mut ids = BTreeSet::new();
        for shard in 0..shards {
            if let Some(bytes) = world_kv_get(&format!("{prefix}/{shard}")) {
                ids.extend(
                    bytes
                        .chunks_exact(8)
                        .map(|c| u64::from_le_bytes(c.try_into().unwrap())),
                );
            }
        }
        Self {
            prefix,
            ids,
            shards,
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.ids.iter().copied()
    }

    /// Put `id` in (`true`) or take it out. A call that changes nothing
    /// writes nothing; `true` when the set changed.
    pub fn set(&mut self, id: u64, present: bool) -> bool {
        let changed = if present {
            self.ids.insert(id)
        } else {
            self.ids.remove(&id)
        };
        if changed {
            self.write_shard(id / IDS_PER_SHARD);
        }
        changed
    }

    fn write_shard(&mut self, shard: u64) {
        let range = shard * IDS_PER_SHARD..(shard + 1).saturating_mul(IDS_PER_SHARD);
        let bytes: Vec<u8> = self
            .ids
            .range(range)
            .flat_map(|id| id.to_le_bytes())
            .collect();
        let key = format!("{}/{shard}", self.prefix);
        if bytes.is_empty() {
            world_kv_delete(&key);
        } else {
            world_kv_set(&key, bytes);
        }
        if shard >= self.shards {
            self.shards = shard + 1;
            world_kv_set(
                &format!("{}/shards", self.prefix),
                self.shards.to_le_bytes().to_vec(),
            );
        }
    }
}
