use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A write that fails must not open the read barrier (a reader would load
/// the record it was replacing), must not be lost to the writes behind it,
/// and must land once the disk takes it again.
#[test]
fn a_failed_write_holds_the_barrier_and_lands_in_order_later() {
    let dir = std::env::temp_dir().join(format!("petramond-save-io-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A directory where the player file goes makes that write fail.
    let blocker = dir.join("players/ada.dat");
    std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let completed = Arc::new((Mutex::new(0u64), Condvar::new()));
    let held = Arc::new(AtomicU64::new(0));
    let writer = {
        let (dir, completed, held) = (dir.clone(), completed.clone(), held.clone());
        std::thread::spawn(move || write_thread(dir, rx, completed, held))
    };
    let player = |byte: u8| IoMsg::SavePlayer {
        name: "ada".into(),
        bytes: vec![byte; 4],
    };
    tx.send((1, player(1))).unwrap();
    tx.send((2, IoMsg::SaveLevel(vec![2; 4]))).unwrap();
    wait_for("both jobs to be held", || held.load(Ordering::Relaxed) == 2);
    assert_eq!(*completed.0.lock().unwrap(), 0);
    assert!(
        !dir.join("level.dat").exists(),
        "nothing overtakes the held job"
    );

    std::fs::remove_dir_all(&blocker).unwrap();
    tx.send((3, IoMsg::Shutdown)).unwrap();
    writer.join().unwrap();
    assert_eq!(*completed.0.lock().unwrap(), 3);
    assert_eq!(held.load(Ordering::Relaxed), 0);
    assert_eq!(std::fs::read(&blocker).unwrap(), vec![1; 4]);
    assert_eq!(std::fs::read(dir.join("level.dat")).unwrap(), vec![2; 4]);
    std::fs::remove_dir_all(&dir).unwrap();
}
