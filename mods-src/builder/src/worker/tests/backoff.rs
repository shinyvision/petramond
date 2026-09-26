//! The three ways work is set aside.

use crate::worker::backoff::{Struck, Tries, Until};
use crate::worker::Task;

#[test]
fn a_wait_holds_until_its_tick_and_is_replaced_or_lifted() {
    let mut until = Until::default();
    until.set(Task::Unit(3), 50);
    assert!(until.holds(&Task::Unit(3), 49));
    assert!(!until.holds(&Task::Unit(3), 50), "lapsed at its tick");
    assert!(!until.holds(&Task::Unit(4), 0), "never set aside");
    assert_eq!(until.until(&Task::Unit(3)), Some(50));

    until.set(Task::Unit(3), 10);
    assert!(
        !until.holds(&Task::Unit(3), 20),
        "a later wait replaces the earlier"
    );
    assert_eq!(until.len(), 1, "lapsed waits are still kept");
    until.lift(&Task::Unit(3));
    assert_eq!((until.until(&Task::Unit(3)), until.len()), (None, 0));
}

#[test]
fn a_strike_stands_until_forgiven() {
    let mut struck = Struck::default();
    struck.strike((Task::Unit(1), [0, 0, 0]));
    struck.strike((Task::Unit(2), [0, 0, 0]));
    assert!(struck.struck(&(Task::Unit(1), [0, 0, 0])));
    assert!(!struck.struck(&(Task::Unit(1), [1, 0, 0])));
    struck.retain(|(t, _)| *t != Task::Unit(1));
    assert!(!struck.struck(&(Task::Unit(1), [0, 0, 0])));
    assert!(struck.struck(&(Task::Unit(2), [0, 0, 0])));
}

#[test]
fn rounds_are_counted_per_key_and_run_out() {
    let mut waits = Tries::default();
    let rounds: Vec<bool> = (0..3).map(|_| waits.within(7usize, 2)).collect();
    assert_eq!(rounds, vec![true, true, false]);
    assert!(waits.within(8, 2), "counted per key");
    assert_eq!(waits.count(7), 4);
    waits.forget(&7);
    assert_eq!(waits.count(7), 1, "forgotten, it starts over");
}

#[test]
fn tries_close_together_count_once() {
    let mut tries = Tries::default();
    assert_eq!(tries.count_spaced(1usize, 10, 40), 1);
    assert_eq!(tries.count_spaced(1, 49, 40), 1, "within the spacing");
    assert_eq!(tries.count_spaced(1, 50, 40), 2);
    assert_eq!(
        tries.count_spaced(1, 60, 40),
        2,
        "spaced from the last counted"
    );
    assert_eq!(tries.count_spaced(2, 60, 40), 1);
}

#[test]
fn counting_never_overflows() {
    let mut tries = Tries::default();
    for _ in 0..300 {
        tries.count([0, 0, 0]);
    }
    assert_eq!(tries.count([0, 0, 0]), u8::MAX);
    assert!(!tries.within([0, 0, 0], 3));
}
