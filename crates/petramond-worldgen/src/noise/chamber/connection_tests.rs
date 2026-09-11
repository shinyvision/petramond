use super::*;
use crate::{data::excavations, noise::cave_field::CaveField};

const CONNECTED: &str = r#"{"excavations":[
 {"excavation":"test:rooms","placement":{"spacing":48,"y":[-48,32]},
  "chamber":{"radius":[6,9],"flatten":0.6,"feather":8},
  "connections":{"radius":[3,4],"flatten":0.8,"feather":8,"bend":0.3}}
]}"#;

#[test]
fn passages_keep_shared_values_when_endpoints_are_outside_the_query() {
    let table = crate::data::underground::test_table(&[]);
    let rows = excavations::test_table(&[CONNECTED], table);
    let field = CaveField::with_tables(42, table, rows);
    let wide = field.chamber_field([-96, -48, -96], [96, 32, 96]);
    let mut checked = 0;
    for passage in &wide.passages {
        let p = passage.points()[4].map(|v| v.round() as i32);
        if p[0].abs() > 64 || p[2].abs() > 64 {
            continue;
        }
        let small = field.chamber_field(p, p);
        for k in [-0.55, 0.0, 0.55] {
            assert_eq!(wide.at(p[0], p[1], p[2], k), small.at(p[0], p[1], p[2], k));
        }
        checked += 1;
    }
    assert!(checked > 0);
    for y in [-49, 33] {
        assert!(field.chamber_field([-64, y, -64], [64, y, 64]).is_empty());
    }
}

#[test]
fn connected_rooms_have_open_passages_after_lattice_interpolation() {
    let table = crate::data::underground::test_table(&[]);
    let rows = excavations::test_table(&[CONNECTED], table);
    for seed in [42, 17, 0xbeef] {
        let field = CaveField::with_tables(seed, table, rows);
        let gathered = field.chamber_field([-24, -48, -24], [24, 32, 24]);
        assert!(!gathered.passages.is_empty());
        let mut queries = Vec::new();
        for passage in gathered.passages.iter().take(3) {
            for pair in passage.points().windows(2) {
                let steps = (0..3)
                    .map(|axis| (pair[1][axis] - pair[0][axis]).abs())
                    .fold(0.0, f64::max)
                    .ceil() as usize;
                for step in 0..=steps {
                    let t = step as f64 / steps.max(1) as f64;
                    let p: [i32; 3] = std::array::from_fn(|axis| {
                        (pair[0][axis] + t * (pair[1][axis] - pair[0][axis])).round() as i32
                    });
                    queries.push((p, 80));
                    queries.push(([p[0], p[1] + 1, p[2]], 80));
                }
            }
        }
        let mut open = Vec::new();
        field.cave_carved_batch(&queries, &mut open);
        for ((p, surface), opened) in queries.iter().zip(open) {
            assert!(opened, "closed passage at {p:?}, seed {seed}");
            assert_eq!(opened, field.cave_carved(p[0], p[1], p[2], *surface));
        }
    }
}

#[test]
fn connections_require_both_rooms_to_pass_admission() {
    let layer = CONNECTED.replace(
        "\"spacing\":48",
        "\"spacing\":48,\"contact\":\"natural_cave\"",
    );
    let table = crate::data::underground::test_table(&[]);
    let rows = excavations::test_table(&[&layer], table);
    let denied = ChamberField::gather(
        &CandidateCache::default(),
        table,
        rows,
        42,
        [[-64, -48, -64], [64, 32, 64]],
        |_, y, _| table.id_at([0.0, 0.0, 0.0, 0.0, 0.0, 0.5], y),
        |_| false,
    );
    assert!(denied.is_empty());
    let admitted = ChamberField::gather(
        &CandidateCache::default(),
        table,
        rows,
        42,
        [[-64, -48, -64], [64, 32, 64]],
        |_, y, _| table.id_at([0.0, 0.0, 0.0, 0.0, 0.0, 0.5], y),
        |_| true,
    );
    assert!(!admitted.passages.is_empty());
}

#[test]
fn a_connected_room_can_join_through_a_directly_anchored_neighbor() {
    let layer = CONNECTED.replace(
        "\"spacing\":48",
        "\"spacing\":48,\"contact\":\"natural_cave\"",
    );
    let table = crate::data::underground::test_table(&[]);
    let rows = excavations::test_table(&[&layer], table);
    let anchor = roll_room(rows.rows[0], 42, 0, 0).unwrap();
    let neighbor = roll_room(rows.rows[0], 42, 1, 0).unwrap();
    let natural = |p: [i32; 3]| p == anchor.center;
    assert!(anchor.contacts(&natural));
    assert!(!neighbor.contacts(&natural));
    let field = ChamberField::gather(
        &CandidateCache::default(),
        table,
        rows,
        42,
        [[-96, -48, -96], [144, 32, 96]],
        |_, y, _| table.id_at([0.0, 0.0, 0.0, 0.0, 0.0, 0.5], y),
        natural,
    );
    assert!(field.rooms.iter().any(|r| r.center == neighbor.center));
    assert!(field.passages.iter().any(|p| {
        p.points().first().unwrap().map(|v| v as i32) == anchor.center
            && p.points().last().unwrap().map(|v| v as i32) == neighbor.center
    }));
}
