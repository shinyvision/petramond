//! Weighing a pillar's trip against the work it lays.

use super::*;

fn trip(moves: Option<u32>, levels: i32, lays: i32, covers: i32) -> Trip {
    Trip {
        moves,
        on_course: false,
        levels,
        lays,
        covers,
    }
}

#[test]
fn a_trip_is_its_walk_climb_and_mount_per_block_laid() {
    let key = trip(Some(10), 3, 2, 1).key();
    assert_eq!(key, (10 + 3 * LEVEL_MOVES + MOUNT_MOVES) * 64 / (4 * 2 + 1));
    // Nothing laid still divides by a block's worth, never by nothing.
    assert_eq!(trip(Some(10), 3, 0, 0).key(), (10 + 3 * LEVEL_MOVES + MOUNT_MOVES) * 64 / 4);
}

#[test]
fn laying_counts_more_than_reaching() {
    let lays = trip(Some(20), 4, 2, 0).key();
    let reaches = trip(Some(20), 4, 0, 2).key();
    assert!(lays < reaches, "{lays} vs {reaches}");
}

#[test]
fn a_foot_no_flood_reaches_is_dear_and_dearer_on_the_design() {
    let ground = trip(None, 2, 1, 0);
    let course = Trip {
        on_course: true,
        ..trip(None, 2, 1, 0)
    };
    let walked = trip(Some(8), 2, 1, 0);
    assert!(walked.key() < ground.key());
    assert!(ground.key() < course.key());
    assert_eq!(ground.key(), (UNWALKED + 2 * LEVEL_MOVES + MOUNT_MOVES) * 64 / 4);
}

#[test]
fn a_perch_covers_the_open_work_within_its_reach() {
    let perch = [0, 3, 0];
    let open = [[1, 3, 0], [0, 4, 1], [40, 3, 0]];
    assert_eq!(covers(perch, &open), 2);
    assert_eq!(covers(perch, &[]), 0);
}
