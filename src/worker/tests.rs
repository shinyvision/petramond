use super::*;

#[test]
fn reprioritizing_waiting_jobs_preserves_fifo_and_submission_identity() {
    let pool = JobPool::new(1);
    let (release, wait) = channel();
    pool.submit(i64::MIN, move || {
        let _ = wait.recv();
    });
    let (send, receive) = channel();
    let mut tickets = Vec::new();
    for (key, tag) in [(1, "old-near"), (100, "new-near-a"), (200, "new-near-b")] {
        let send = send.clone();
        tickets.push(pool.submit(key, move || {
            send.send(tag).unwrap();
        }));
    }
    let foreign = JobPool::inline().submit(0, || {});
    pool.reprioritize([
        (tickets[0], 300),
        (tickets[1], 2),
        (tickets[2], 2),
        (foreign, -1),
    ]);
    release.send(()).unwrap();
    let order: Vec<_> = (0..3)
        .map(|_| {
            receive
                .recv_timeout(petramond_util::test_time::TEST_HARD_DEADLINE)
                .unwrap()
        })
        .collect();
    assert_eq!(order, ["new-near-a", "new-near-b", "old-near"]);
}

#[test]
fn removing_waiting_jobs_leaves_running_and_finished_work_owned_by_the_caller() {
    let pool = JobPool::new(1);
    let (release, wait) = channel();
    let (started, starting) = channel();
    let (done, completed) = channel();
    let running = pool.submit(0, move || {
        started.send(()).unwrap();
        let _ = wait.recv();
        done.send(()).unwrap();
    });
    starting
        .recv_timeout(petramond_util::test_time::TEST_HARD_DEADLINE)
        .unwrap();
    let (send, receive) = channel();
    let waiting = pool.submit(1, move || {
        send.send(()).unwrap();
    });
    let removed = pool.remove_queued([running, waiting]);
    release.send(()).unwrap();
    completed
        .recv_timeout(petramond_util::test_time::TEST_HARD_DEADLINE)
        .unwrap();
    assert_eq!(removed, FxHashSet::from_iter([waiting]));
    assert!(receive.recv().is_err(), "removed closures must be released");
    assert!(pool.remove_queued([running, waiting]).is_empty());
}

#[test]
fn pool_runs_lowest_key_first_and_fifo_on_ties() {
    // One worker so execution order IS pop order.
    let pool = JobPool::new(1);
    let order = Arc::new(Mutex::new(Vec::new()));
    // Park the worker on a first job so the rest queue up behind it and get
    // priority-ordered rather than raced one-by-one.
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    {
        let gate = gate.clone();
        pool.submit(i64::MIN, move || {
            let (lock, cv) = &*gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        });
    }
    for (key, tag) in [(50, "far"), (10, "near-a"), (10, "near-b"), (30, "mid")] {
        let order = order.clone();
        pool.submit(key, move || order.lock().unwrap().push(tag));
    }
    // Largest key = runs last; signals that everything before it completed
    // (dropping the pool discards unstarted jobs, so wait before dropping).
    let (done_tx, done_rx) = channel::<()>();
    pool.submit(i64::MAX, move || {
        let _ = done_tx.send(());
    });
    {
        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    done_rx.recv().unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        vec!["near-a", "near-b", "mid", "far"]
    );
}

/// A worker runs each job under the registry its SUBMITTER reads, not the
/// worker thread's own: a pool serves whichever world queued the work.
#[test]
fn jobs_run_under_the_submitters_content_registry() {
    use petramond_world::content::{self, Content};
    let no_mods = std::env::temp_dir().join(format!("petramond-pool-{}", std::process::id()));
    let own = content::test_support::with_mods(&no_mods);
    let pool = JobPool::new(1);
    let (tx, rx) = channel::<bool>();
    {
        let _pin = content::pin(own);
        let tx = tx.clone();
        pool.submit(0, move || {
            let _ = tx.send(Content::current().same(own));
        });
    }
    pool.submit(1, move || {
        let _ = tx.send(Content::current().same(own));
    });
    assert!(rx.recv().unwrap(), "the pinned submitter's registry rides the job");
    assert!(
        !rx.recv().unwrap(),
        "an unpinned submitter's job reads the process registry"
    );
}

#[test]
fn a_panicking_job_reports_its_failure_through_its_slot() {
    let pool = JobPool::new(1);
    let (tx, rx) = channel::<Result<u32, SectionPos>>();
    let at = SectionPos::new(1, 2, 3);
    let slot = ReportSlot::new(tx.clone(), Err(at), "test", at);
    pool.submit(0, move || {
        let _slot = slot;
        panic!("injected job panic");
    });
    let ok_slot = ReportSlot::new(tx, Err(at), "test", at);
    pool.submit(1, move || ok_slot.complete(Ok(7)));
    let deadline = petramond_util::test_time::TEST_HARD_DEADLINE;
    assert_eq!(rx.recv_timeout(deadline).unwrap(), Err(at));
    assert_eq!(
        rx.recv_timeout(deadline).unwrap(),
        Ok(7),
        "the worker survives the panic and keeps running jobs"
    );
}

#[test]
fn an_inline_pool_contains_a_panicking_job_and_still_reports() {
    let pool = JobPool::inline();
    let (tx, rx) = channel::<bool>();
    let slot = ReportSlot::new(tx, false, "test", SectionPos::new(0, 0, 0));
    pool.submit(0, move || {
        let _slot = slot;
        panic!("injected inline panic");
    });
    assert_eq!(rx.try_recv(), Ok(false), "the caller got the failure, not the unwind");
}

#[test]
fn a_job_discarded_unstarted_reports_its_failure() {
    let pool = JobPool::new(1);
    let (release, wait) = channel::<()>();
    let (started, starting) = channel::<()>();
    pool.submit(i64::MIN, move || {
        started.send(()).unwrap();
        let _ = wait.recv();
    });
    starting
        .recv_timeout(petramond_util::test_time::TEST_HARD_DEADLINE)
        .unwrap();
    let (tx, rx) = channel::<bool>();
    let slot = ReportSlot::new(tx, false, "test", SectionPos::new(0, 0, 0));
    let ticket = pool.submit(0, move || slot.complete(true));
    assert_eq!(pool.remove_queued([ticket]).len(), 1);
    assert_eq!(rx.try_recv(), Ok(false));
    release.send(()).unwrap();
}

#[test]
fn rekeying_and_removing_touch_only_the_named_jobs() {
    let pool = JobPool::new(1);
    let (release, wait) = channel::<()>();
    let (started, starting) = channel::<()>();
    pool.submit(i64::MIN, move || {
        started.send(()).unwrap();
        let _ = wait.recv();
    });
    starting
        .recv_timeout(petramond_util::test_time::TEST_HARD_DEADLINE)
        .unwrap();
    let (send, receive) = channel();
    let tickets: Vec<JobTicket> = (0..1000i64)
        .map(|i| {
            let send = send.clone();
            pool.submit(i, move || send.send(i).unwrap())
        })
        .collect();
    assert_eq!(pool.queued_len(), 1000);
    // Re-key three jobs to the front and drop one: the other jobs keep their
    // places.
    pool.reprioritize([(tickets[900], -3), (tickets[901], -2), (tickets[902], -1)]);
    assert_eq!(pool.remove_queued([tickets[0]]).len(), 1);
    assert_eq!(pool.queued_len(), 999);
    release.send(()).unwrap();
    let deadline = petramond_util::test_time::TEST_HARD_DEADLINE;
    let first: Vec<i64> = (0..5)
        .map(|_| receive.recv_timeout(deadline).unwrap())
        .collect();
    assert_eq!(first, vec![900, 901, 902, 1, 2]);
}

#[test]
fn default_threads_never_exceeds_the_machine() {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let threads = JobPool::default_threads();
    assert!(threads >= 1 && threads <= n);
}
