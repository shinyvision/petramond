use super::*;
use crate::mob::{Mob, MobRng};

fn mob(id: u64, x: f64, z: f64, active: bool) -> AiMob {
    AiMob {
        id,
        kind: Mob::Sheep,
        pos: WorldPos::new(x, 64.0, z),
        active,
        tags: Default::default(),
    }
}

/// A deterministic scatter straddling the origin (negative columns included)
/// and several column boundaries.
fn scatter(n: u64) -> Vec<AiMob> {
    let mut rng = MobRng::new(7);
    (0..n)
        .map(|i| {
            let x = f64::from(rng.next_f32()) * 200.0 - 100.0;
            let z = f64::from(rng.next_f32()) * 200.0 - 100.0;
            mob(1000 + i, x, z, i % 7 != 0)
        })
        .collect()
}

#[test]
fn near_matches_a_brute_force_scan_exactly() {
    let mobs = scatter(400);
    let snapshot = MobSnapshot::from_mobs(mobs.clone());
    let probes = [
        (WorldPos::new(0.0, 64.0, 0.0), 5.0),
        (WorldPos::new(-15.9, 64.0, 16.1), 12.0),
        (WorldPos::new(47.0, 64.0, -33.0), 32.5),
        (WorldPos::new(-100.0, 64.0, -100.0), 1.0),
        (WorldPos::new(3.0, 64.0, 3.0), 0.0),
        (WorldPos::new(0.0, 64.0, 0.0), 500.0),
    ];
    for (pos, reach) in probes {
        let mut got: Vec<usize> = snapshot.near(pos, reach).map(|(i, _)| i).collect();
        got.sort_unstable();
        let r = f64::from(reach);
        let want: Vec<usize> = mobs
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.active && (m.pos.x - pos.x).abs() <= r && (m.pos.z - pos.z).abs() <= r
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(got, want, "query at {pos:?} r={reach}");
    }
}

#[test]
fn near_yields_each_mob_once_in_a_deterministic_order() {
    let snapshot = MobSnapshot::from_mobs(scatter(300));
    let pos = WorldPos::new(10.0, 64.0, -10.0);
    let first: Vec<usize> = snapshot.near(pos, 40.0).map(|(i, _)| i).collect();
    let second: Vec<usize> = snapshot.near(pos, 40.0).map(|(i, _)| i).collect();
    assert_eq!(first, second);
    let mut unique = first.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), first.len(), "no mob is visited twice");
}

#[test]
fn id_lookup_resolves_every_mob_and_live_skips_inactive_ones() {
    let mobs = vec![
        mob(5, 1.0, 1.0, true),
        mob(9, 2.0, 2.0, false),
        mob(3, -40.0, 7.0, true),
    ];
    let snapshot = MobSnapshot::from_mobs(mobs);
    assert_eq!(snapshot.by_id(9).map(|(i, m)| (i, m.id)), Some((1, 9)));
    assert_eq!(snapshot.by_id(3).map(|(i, _)| i), Some(2));
    assert!(snapshot.by_id(4).is_none());
    assert!(snapshot.live(5).is_some());
    assert!(
        snapshot.live(9).is_none(),
        "an inactive mob is not targetable"
    );
    assert!(
        snapshot
            .near(WorldPos::new(2.0, 64.0, 2.0), 3.0)
            .all(|(_, m)| m.id != 9),
        "inactive mobs are not in the grid"
    );
}

#[test]
fn rebuild_replaces_the_previous_contents() {
    let mut snapshot = MobSnapshot::from_mobs(scatter(50));
    snapshot.rebuild([mob(77, 0.5, 0.5, true)]);
    assert!(snapshot.by_id(1001).is_none());
    let found: Vec<u64> = snapshot
        .near(WorldPos::new(0.0, 64.0, 0.0), 100.0)
        .map(|(_, m)| m.id)
        .collect();
    assert_eq!(found, vec![77]);
    assert!(snapshot.max_half_extent() > 0.0);
    snapshot.rebuild([]);
    assert!(snapshot.by_id(77).is_none());
    assert_eq!(snapshot.max_half_extent(), 0.0);
}
