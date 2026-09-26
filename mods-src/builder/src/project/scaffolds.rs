//! Each project's standing scaffold cells, kept beside its record rather than
//! inside it.
//!
//! The list grows with the build — a tall one leaves hundreds of scaffolds up
//! at once — and a world-KV value is capped, so a list inside the project
//! record would give large builds a hidden ceiling past which the record
//! cannot be saved at all. Here the list is sharded by position: a push
//! rewrites the last shard, and only a removal early in the list rewrites
//! the shards after it.
//!
//! World KV keys are `"builder:scaffolds/{id}/{shard}"` (twelve LE bytes per
//! cell) plus `"builder:scaffolds/{id}/shards"`, the shard count. Like the
//! SDK's record stores this is the mod's frame, so it talks to mod-sdk
//! directly.

use mod_sdk::{world_kv_delete, world_kv_get, world_kv_set, KV_MAX_VALUE_BYTES};

use super::ProjectId;

const PREFIX: &str = "builder:scaffolds";
const CELL_BYTES: usize = 12;
const CELLS_PER_SHARD: usize = 4096;
const _: () = assert!(CELLS_PER_SHARD * CELL_BYTES <= KV_MAX_VALUE_BYTES);

fn shard_key(id: ProjectId, shard: usize) -> String {
    format!("{PREFIX}/{id}/{shard}")
}

fn count_key(id: ProjectId) -> String {
    format!("{PREFIX}/{id}/shards")
}

/// The stored list, or `None` when this project's scaffolds were never
/// stored here (a new project, or one saved before they moved out of its
/// record).
pub(super) fn load(id: ProjectId) -> Option<Vec<[i32; 3]>> {
    let shards = world_kv_get(&count_key(id))?;
    let shards = u32::from_le_bytes(shards.try_into().ok()?) as usize;
    let mut cells = Vec::new();
    for shard in 0..shards {
        cells.extend(world_kv_get(&shard_key(id, shard)).iter().flat_map(|bytes| {
            bytes.chunks_exact(CELL_BYTES).map(|c| {
                let axis = |a: usize| i32::from_le_bytes(c[a * 4..a * 4 + 4].try_into().unwrap());
                [axis(0), axis(1), axis(2)]
            })
        }));
    }
    Some(cells)
}

/// Store `after` over a stored `before`, writing only the shards that
/// changed. `fresh` = nothing is stored yet, so the count is written even
/// when it does not change.
pub(super) fn save(id: ProjectId, before: &[[i32; 3]], after: &[[i32; 3]], fresh: bool) {
    if before == after && !fresh {
        return;
    }
    let same = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let (old, new) = (shards_of(before.len()), shards_of(after.len()));
    for shard in same / CELLS_PER_SHARD..new {
        let end = after.len().min((shard + 1) * CELLS_PER_SHARD);
        let bytes: Vec<u8> = after[shard * CELLS_PER_SHARD..end]
            .iter()
            .flat_map(|c| c.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        world_kv_set(&shard_key(id, shard), bytes);
    }
    for shard in new..old {
        world_kv_delete(&shard_key(id, shard));
    }
    if fresh || new != old {
        world_kv_set(&count_key(id), (new as u32).to_le_bytes().to_vec());
    }
}

fn shards_of(cells: usize) -> usize {
    cells.div_ceil(CELLS_PER_SHARD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_stored_across_shards_and_reads_back_in_order() {
        let _session = crate::testing::Session::flat(1);
        assert_eq!(load(3), None, "nothing stored yet");
        let cells: Vec<[i32; 3]> = (0..CELLS_PER_SHARD as i32 + 5).map(|i| [i, -i, 7]).collect();
        save(3, &[], &cells, true);
        assert_eq!(load(3).as_deref(), Some(&cells[..]));

        // A removal near the front shifts everything after it.
        let mut fewer = cells.clone();
        fewer.remove(1);
        save(3, &cells, &fewer, false);
        assert_eq!(load(3).as_deref(), Some(&fewer[..]));

        // Emptied, it is stored as empty, not as never stored.
        save(3, &fewer, &[], false);
        assert_eq!(load(3), Some(Vec::new()));
        assert_eq!(world_kv_get(&shard_key(3, 0)), None, "no shard left behind");
    }
}
