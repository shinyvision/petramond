use super::*;

fn p(x: i32) -> IVec3 {
    IVec3::new(x, 0, 0)
}

#[test]
fn readers_follow_the_log_independently() {
    let mut log = ChangeLog::default();
    let (mut a, mut b) = (log.end(), log.end());
    let read = |log: &ChangeLog, seq: &mut u64| {
        let out = log.since(*seq);
        *seq = log.end();
        out
    };
    log.push(p(1), true);
    assert_eq!(read(&log, &mut a), (vec![p(1)], false));
    log.push(p(2), false);
    assert_eq!(read(&log, &mut b), (vec![p(1), p(2)], false));
    assert_eq!(read(&log, &mut a), (vec![p(2)], false));
    assert_eq!(read(&log, &mut a), (vec![], false));
    assert_eq!(read(&log, &mut b), (vec![], false));
}

#[test]
fn a_reader_the_log_slid_past_is_told_it_lost_some() {
    let mut log = ChangeLog::default();
    let late = log.end();
    for i in 0..(CHANGE_LOG_CAP as i32 * 2) {
        log.push(p(i), false);
    }
    assert!(log.window.len() <= CHANGE_LOG_CAP);
    assert!(log.since(late).1, "the log slid past the reader");
    assert!(log.nav_since(late).1, "loss is loss whatever the filter");
    assert!(log.since(log.end() + 1).1, "a place from another numbering");
    let current = log.end();
    log.push(p(-1), true);
    assert_eq!(log.since(current), (vec![p(-1)], false));
}

#[test]
fn the_nav_view_skips_other_changes_but_shares_the_numbering() {
    let mut log = ChangeLog::default();
    let start = log.end();
    let revision = log.nav_revision();
    log.push(p(1), false);
    assert_eq!(log.nav_revision(), revision);
    log.push(p(2), true);
    assert_ne!(log.nav_revision(), revision);
    assert_eq!(log.nav_since(start), (vec![p(2)], false));
    assert_eq!(log.nav_since(start + 2), (vec![], false));
}

#[test]
fn scheduled_queue_dedups_and_pops_due_checks_in_schedule_order() {
    let mut q = ScheduledQueue::default();
    assert!(q.schedule(p(1), 5));
    assert!(q.schedule(p(2), 3));
    assert!(q.schedule(p(3), 5));
    assert!(!q.schedule(p(1), 1), "first schedule wins");
    assert_eq!(q.len(), 3);
    assert_eq!(q.pop_due(2), None, "nothing due yet");
    assert_eq!(q.due_count(5), 3);
    assert_eq!(q.pop_due(5), Some(p(2)));
    assert_eq!(q.pop_due(5), Some(p(1)));
    assert!(!q.contains(p(1)));
    assert!(q.schedule(p(1), 5), "a popped cell can be scheduled again");
    assert_eq!(q.pop_due(5), Some(p(3)));
    assert_eq!(q.pop_due(5), Some(p(1)));
    assert!(q.is_empty());
}

/// A check left behind by a budgeted drain keeps its place ahead of
/// anything scheduled later, so carry-over drains oldest-first.
#[test]
fn scheduled_queue_carry_over_stays_ahead_of_later_schedules() {
    let mut q = ScheduledQueue::default();
    q.schedule(p(1), 1);
    q.schedule(p(2), 1);
    assert_eq!(q.pop_due(1), Some(p(1)));
    // Budget ran out; a later tick schedules fresh work due at once.
    q.schedule(p(3), 2);
    assert_eq!(q.pop_due(2), Some(p(2)));
    assert_eq!(q.pop_due(2), Some(p(3)));
}
