//! Player-anchored mob sweeps: the live mobs near any of several anchors,
//! each mob once, from as few radius queries as the anchors allow.

use std::collections::HashSet;

use mod_api::MobSnapshot;

use crate::mobs_in_radius;

/// Every live mob within `radius` of ANY of `anchors` (typically the
/// connected players), each exactly ONCE, in first-seen order — anchor
/// order, then the host's storage order within an anchor's reply. The order
/// is what a per-anchor loop that skips already-seen ids would visit, so a
/// sweep keeps its determinism; what it drops is the duplicate work where
/// the anchors' ranges overlap. Anchors at the very same point (co-op
/// players sharing a seat or a spawn) share one query.
pub fn mobs_near_any(anchors: &[[f64; 3]], radius: f32) -> Vec<MobSnapshot> {
    first_seen(anchors, |at| mobs_in_radius(at, radius))
}

/// The sweep over any radius query (the host call, or a test's fake).
fn first_seen(
    anchors: &[[f64; 3]],
    mut query: impl FnMut([f64; 3]) -> Vec<MobSnapshot>,
) -> Vec<MobSnapshot> {
    let mut seen = HashSet::new();
    let mut queried: Vec<[f64; 3]> = Vec::with_capacity(anchors.len());
    let mut out = Vec::new();
    for &at in anchors {
        if queried.contains(&at) {
            continue;
        }
        queried.push(at);
        out.extend(query(at).into_iter().filter(|m| seen.insert(m.id)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::MobId;

    fn mob(id: u64) -> MobSnapshot {
        MobSnapshot {
            index: id as u32,
            kind: MobId(0),
            pos: [0.0; 3],
            health: 1.0,
            id,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            vel: [0.0; 3],
            on_ground: true,
            moving: false,
            half_width: 0.5,
            height: 1.0,
            half_length: 0.5,
            entombed: false,
            conditions: Vec::new(),
        }
    }

    #[test]
    fn overlapping_anchors_yield_each_mob_once_in_first_seen_order() {
        let mut queries = 0;
        let found = first_seen(&[[0.0; 3], [10.0, 0.0, 0.0], [0.0; 3]], |at| {
            queries += 1;
            if at[0] == 0.0 {
                vec![mob(3), mob(1)]
            } else {
                vec![mob(1), mob(7), mob(3)]
            }
        });
        let ids: Vec<u64> = found.iter().map(|m| m.id).collect();
        assert_eq!(ids, [3, 1, 7], "anchor order, then reply order, no repeats");
        assert_eq!(queries, 2, "a repeated anchor is not queried again");
    }
}
