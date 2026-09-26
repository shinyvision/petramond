use super::*;
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
    let ada = crate::net::identity::PlayerKey([0xAD; 32]);
    let blocker = dir.join(format!("players/{ada}.dat"));
    std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let reads = ReadQueues::new(1);
    let held = Arc::new(AtomicU64::new(0));
    let writer = {
        let (dir, reads, held) = (dir.clone(), reads.clone(), held.clone());
        std::thread::spawn(move || {
            write_thread(dir, rx, reads, held, Arc::new(Palette::identity()))
        })
    };
    let player = |byte: u8| IoMsg::SavePlayer {
        key: ada,
        bytes: vec![byte; 4],
    };
    tx.send((1, player(1))).unwrap();
    tx.send((2, IoMsg::SaveLevel(vec![2; 4]))).unwrap();
    wait_for("both writes to be held", || {
        held.load(Ordering::Relaxed) == 2
    });
    assert_eq!(reads.completed(), 0);
    assert!(
        !dir.join("level.dat").exists(),
        "nothing overtakes the held write"
    );

    std::fs::remove_dir_all(&blocker).unwrap();
    tx.send((3, IoMsg::Shutdown)).unwrap();
    writer.join().unwrap();
    assert_eq!(reads.completed(), 3);
    assert_eq!(held.load(Ordering::Relaxed), 0);
    assert_eq!(std::fs::read(&blocker).unwrap(), vec![1; 4]);
    assert_eq!(std::fs::read(dir.join("level.dat")).unwrap(), vec![2; 4]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A backlog merges into one job: the newest version of each file and of
/// each region slot wins, other slots of a region survive, and the job
/// completes up to the newest message folded in.
#[test]
fn a_backlog_merges_into_one_job_where_later_writes_win() {
    let dir = std::env::temp_dir().join(format!("petramond-save-merge-{}", std::process::id()));
    let pal = Palette::identity();
    let ada = crate::net::identity::PlayerKey([0xAD; 32]);
    let section = |x: i32, stone: bool| {
        let mut s = petramond_world::section::Section::new(x, 4, 0);
        if stone {
            s.set_block(1, 1, 1, petramond_world::block::Block::Stone);
        }
        super::super::SectionSnapshot::from_section(&s)
    };
    let sections = |snaps| IoMsg::SaveSections {
        store: SectionStore::Authoritative,
        records: EncodeSlot::new(snaps),
    };

    let mut job = Job::new();
    job.absorb(
        4,
        sections(vec![section(0, false), section(1, false)]),
        &dir,
        &pal,
        false,
    );
    job.absorb(
        5,
        IoMsg::SavePlayer {
            key: ada,
            bytes: vec![1],
        },
        &dir,
        &pal,
        false,
    );
    job.absorb(
        7,
        IoMsg::Batch(vec![
            sections(vec![section(1, true)]),
            IoMsg::SavePlayer {
                key: ada,
                bytes: vec![2],
            },
        ]),
        &dir,
        &pal,
        false,
    );
    assert_eq!((job.seq, job.msgs), (7, 3));
    assert_eq!(job.entries.len(), 2, "one region entry, one player file");
    let Entry::Region { records, .. } = &job.entries[0] else {
        panic!("the region entry comes first");
    };
    let slot = |x| region::local_index(SectionPos::new(x, 4, 0));
    let bytes_of = |x| {
        records
            .iter()
            .find(|(lidx, _)| *lidx == slot(x))
            .map(|(_, b)| b.clone())
    };
    assert_eq!(records.len(), 2);
    assert_eq!(
        bytes_of(1),
        Some(codec_bytes(&section(1, true), &pal)),
        "the later version of a slot wins"
    );
    assert_eq!(bytes_of(0), Some(codec_bytes(&section(0, false), &pal)));
    assert!(matches!(
        &job.entries[1],
        Entry::File { bytes, .. } if bytes == &vec![2]
    ));
}

fn codec_bytes(snap: &super::super::SectionSnapshot, pal: &Palette) -> Vec<u8> {
    super::super::codec::encode_snapshot(snap, pal)
}

/// While a job keeps failing, rebuildable cache writes are not held in
/// memory behind it.
#[test]
fn a_failing_job_drops_cache_writes() {
    let dir = std::env::temp_dir().join(format!("petramond-save-cache-{}", std::process::id()));
    let pal = Palette::identity();
    let mut job = Job::new();
    job.absorb(1, IoMsg::SaveColumnGens(Vec::new()), &dir, &pal, false);
    assert_eq!(job.caches.len(), 1);
    job.absorb(2, IoMsg::SaveColumnGens(Vec::new()), &dir, &pal, true);
    assert_eq!(job.caches.len(), 1, "dropped while failing");
}
