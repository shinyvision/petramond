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
