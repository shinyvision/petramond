//! The rest an attracting area takes after it has produced a visitor.
//!
//! Attraction alone makes a planted field a faucet on a timer the player
//! controls: leave, come back, and the stand has pulled in a fresh animal
//! to butcher. So once a field draws one in, the AREA around that field —
//! every 16×16 column within the attraction's own neighbourhood — sits out
//! attraction for [`REST_TICKS`], and a crop in a resting column does not
//! roll at all.
//!
//! The rest is world KV (one row per resting column, the expiry tick as its
//! value), so it rides the save like the tick it is measured against: a
//! reload neither forfeits it nor restarts it. Rows are deleted lazily by
//! the first roll that finds them expired — a column only ever holds a row
//! while it is, or was last, resting.

use mod_sdk::*;

/// How long an area rests after producing a visitor: an hour of play.
pub const REST_TICKS: u64 = 72_000;

const KEY_PREFIX: &str = "farming:attract_rest";

/// Whether the column holding `pos` is still resting. An expired row is
/// deleted on the way out.
pub fn resting(pos: [i32; 3]) -> bool {
    let key = key(column(pos));
    let Some(expiry) = world_kv_get(&key).and_then(|b| decode(&b)) else {
        return false;
    };
    if current_tick() < expiry {
        return true;
    }
    world_kv_delete(&key);
    false
}

/// Put every column within `radius` blocks of `pos` to rest from now.
pub fn begin(pos: [i32; 3], radius: i32) {
    let expiry = current_tick() + REST_TICKS;
    for col in columns_within(pos, radius) {
        world_kv_set(&key(col), expiry.to_le_bytes().to_vec());
    }
}

fn key([cx, cz]: [i32; 2]) -> String {
    format!("{KEY_PREFIX}/{cx}/{cz}")
}

fn decode(bytes: &[u8]) -> Option<u64> {
    bytes.try_into().ok().map(u64::from_le_bytes)
}

/// The 16×16 column holding a block (floor division, so negative
/// coordinates land in their own column, not their neighbour's).
fn column(pos: [i32; 3]) -> [i32; 2] {
    [pos[0] >> 4, pos[2] >> 4]
}

/// Every column the square `pos ± radius` overlaps.
fn columns_within(pos: [i32; 3], radius: i32) -> impl Iterator<Item = [i32; 2]> {
    let [x0, z0] = column([pos[0] - radius, 0, pos[2] - radius]);
    let [x1, z1] = column([pos[0] + radius, 0, pos[2] + radius]);
    (x0..=x1).flat_map(move |cx| (z0..=z1).map(move |cz| [cx, cz]))
}

#[cfg(test)]
mod tests {
    use super::{column, columns_within};

    #[test]
    fn negative_coordinates_floor_into_their_own_column() {
        assert_eq!(column([-1, 0, -16]), [-1, -1]);
        assert_eq!(column([-17, 0, 15]), [-2, 0]);
    }

    #[test]
    fn footprint_covers_every_column_the_radius_touches() {
        // A crop at the corner of column (0,0) with the headcount radius
        // reaches into all four neighbours on the negative side.
        let cols: Vec<_> = columns_within([1, 64, 1], 16).collect();
        assert_eq!(cols.len(), 9);
        assert!(cols.contains(&[-1, -1]));
        assert!(cols.contains(&[1, 1]));
        assert!(!cols.contains(&[2, 0]));
    }
}
