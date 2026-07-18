//! The one-byte cell-KV counter this pack's "N random ticks then act"
//! blocks share (farmland idle decay, fertilized-grass fertility).

use mod_sdk::*;

/// Read the one-byte counter under `key` in the cell's KV and return it
/// bumped by one (absent = 0 → 1). Deliberately does NOT write: the caller
/// decides when — farmland must persist its count AFTER the wet/dry
/// reconcile swap (a block write clears cell KV), and a caller acting at
/// the threshold never writes at all (the acting block write clears it).
pub fn kv_counter_bump(pos: [i32; 3], key: &str) -> u8 {
    section_kv_get(pos, key)
        .and_then(|b| b.first().copied())
        .unwrap_or(0)
        + 1
}
