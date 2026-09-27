use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use petramond_util::test_dirs::TestScratchDir;

use super::*;

fn file(dir: &Path, rel: &str) -> FileRef {
    FileRef::new(Arc::from(dir.join("files")), rel)
}

type Answer<T> = Receiver<Result<T, String>>;

fn answer<T: Send + 'static>() -> (Hook<T>, Answer<T>) {
    let (tx, rx) = mpsc::channel();
    let hook: Hook<T> = Box::new(move |result| {
        let _ = tx.send(result);
    });
    (hook, rx)
}

fn arrived<T>(rx: &Answer<T>) -> Result<T, String> {
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the operation finished")
}

fn pending<T>(rx: &Answer<T>) -> bool {
    matches!(rx.try_recv(), Err(TryRecvError::Empty))
}

#[test]
fn a_read_sees_its_own_queued_writes_and_never_waits_behind_unrelated_ones() {
    let dir = TestScratchDir::new("mod-files-read-order");
    let log = file(&dir, "logs/day.log");
    let (done, landed) = answer();
    append(&log, b"hello world".to_vec(), done).unwrap();
    assert_eq!(arrived(&landed), Ok([0, 11]));

    let (done, record_landed) = answer();
    let slot = record(&log, done).unwrap();
    let (done, write_landed) = answer();
    write(&log, 0, b"HELLO".to_vec(), false, done).unwrap();
    let (done, tail_landed) = answer();
    append(&log, b"abc".to_vec(), done).unwrap();

    let (done, untouched) = answer();
    read(&log, 6, 5, done);
    assert_eq!(arrived(&untouched), Ok(b"world".to_vec()));
    let (done, overwritten) = answer();
    read(&log, 0, 5, done);
    let (done, queued) = answer();
    read(&log, 11, 100, done);
    assert!(pending(&overwritten) && pending(&queued));
    let info = stat(&log).unwrap();
    assert_eq!((info.written, info.len), (11, 14));

    slot.fill(vec![b"!".to_vec(), b"!".to_vec()]);
    assert_eq!(arrived(&record_landed), Ok([11, 2]));
    assert_eq!(arrived(&write_landed), Ok([0, 5]));
    assert_eq!(arrived(&tail_landed), Ok([13, 3]));
    assert_eq!(arrived(&overwritten), Ok(b"HELLO".to_vec()));
    assert_eq!(arrived(&queued), Ok(b"!!abc".to_vec()));
}

#[test]
fn a_failed_record_fails_the_records_queued_behind_it() {
    let dir = TestScratchDir::new("mod-files-record-hole");
    let log = file(&dir, "events.pmc");
    let (done, first) = answer();
    let broken = record(&log, done).unwrap();
    let (done, second) = answer();
    let behind = record(&log, done).unwrap();
    let (done, own) = answer();
    append(&log, b"mod".to_vec(), done).unwrap();
    behind.fill(vec![b"second".to_vec()]);
    broken.fail("encoding failed".into());
    assert!(arrived(&first).is_err());
    assert!(arrived(&second).is_err(), "no record lands past the hole");
    assert_eq!(arrived(&own), Ok([0, 3]));

    let (done, later) = answer();
    record(&log, done).unwrap().fill(vec![b"later".to_vec()]);
    assert_eq!(arrived(&later), Ok([3, 5]));
}

#[test]
fn a_bucket_is_swept_of_what_an_earlier_run_left_hidden() {
    let dir = TestScratchDir::new("mod-files-sweep");
    let me = std::process::id();
    let other = me.wrapping_add(1);
    let recordings = dir.join("recordings");
    std::fs::create_dir_all(recordings.join(format!(".trash-{other}-3")).join("r1")).unwrap();
    let stale = [
        format!(".clip.mp4.{other}-0.video.partial"),
        format!(".clip.mp4.{other}-0.partial"),
    ];
    let kept = [
        format!(".trash-{me}-0"),
        format!(".clip.mp4.{me}-1.pcm.partial"),
        "clip.mp4".to_owned(),
    ];
    for name in stale.iter().chain(&kept) {
        std::fs::write(recordings.join(name), b"x").unwrap();
    }
    paths::sweep(&dir, me);
    let mut left: Vec<String> = std::fs::read_dir(&recordings)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    let mut want = kept.to_vec();
    want.sort();
    assert_eq!(left, want);
}

#[test]
fn a_delete_ends_incarnations_at_the_call_and_fails_waiting_reads_by_name() {
    let dir = TestScratchDir::new("mod-files-delete");
    let changes = subscribe();
    let (kept, doomed) = (file(&dir, "a/kept"), file(&dir, "rec/r3/events.pmc"));
    let (done, landed) = answer();
    append(&kept, vec![7; 4], done).unwrap();
    arrived(&landed).unwrap();
    let kept_incarnation = kept.incarnation();
    let (done, landed) = answer();
    rename(&kept, &kept.sibling("b/kept"), done).unwrap();
    arrived(&landed).unwrap();
    assert_eq!(kept.sibling("b/kept").incarnation(), kept_incarnation);

    let (done, _) = answer();
    let slot = record(&doomed, done).unwrap();
    let incarnation = doomed.incarnation();
    let (done, waiting) = answer();
    read(&doomed, 0, 8, done);
    let (done, deleted) = answer();
    delete(&file(&dir, "rec"), done).unwrap();
    assert_eq!(
        arrived(&waiting),
        Err("rec/r3/events.pmc was deleted".into())
    );
    assert!(changes.try_iter().any(|change| matches!(
        change,
        FileChange::Ended { incarnations, .. } if incarnations == vec![incarnation]
    )));
    assert!(
        pending(&deleted),
        "the delete waits for the record queued first"
    );
    slot.fill(vec![vec![1; 8]]);
    assert_eq!(arrived(&deleted), Ok(()));
    assert!(!dir.join("files").join("rec").exists());
    assert_ne!(doomed.incarnation(), incarnation);
}

#[test]
fn a_create_that_collides_by_case_is_refused() {
    let dir = TestScratchDir::new("mod-files-case");
    let put = |rel: &str| {
        let (done, landed) = answer();
        append(&file(&dir, rel), vec![1], done).unwrap();
        arrived(&landed)
    };
    assert!(put("Shots/A.png").is_ok());
    assert!(put("Shots/A.png").is_ok());
    assert!(put("Shots/b.png").is_ok());
    for clash in ["Shots/a.png", "shots/c.png", "Shots/B.PNG"] {
        let refused = put(clash).unwrap_err();
        assert!(
            refused.contains("differs only by case"),
            "{clash}: {refused}"
        );
    }
}

#[test]
fn list_paging_continues_exactly_and_never_shows_hidden_names() {
    let dir = TestScratchDir::new("mod-files-list");
    let files_dir = dir.join("files").join("shots");
    std::fs::create_dir_all(files_dir.join("sub")).unwrap();
    for name in ["b.png", "a.png", "ä.png", "c d.png", ".trash-1-0", ".part"] {
        std::fs::write(files_dir.join(name), name).unwrap();
    }
    let page = |after: Option<String>, max_bytes: u64| {
        let (done, listed) = answer();
        list(&file(&dir, "shots"), after, max_bytes, done);
        arrived(&listed).unwrap()
    };
    let (mut seen, mut after) = (Vec::new(), None);
    loop {
        let (entries, more) = page(after.clone(), 1);
        assert_eq!(entries.len(), 1, "one entry whatever the budget");
        after = Some(entries[0].name.clone());
        seen.push(entries[0].name.clone());
        if !more {
            break;
        }
    }
    assert_eq!(seen, ["a.png", "b.png", "c d.png", "sub", "ä.png"]);
    let (entries, more) = page(None, u64::MAX);
    assert!(!more);
    assert_eq!(entries.len(), 5);
    assert!(entries.iter().find(|e| e.name == "sub").unwrap().dir);
    assert_eq!(entries[0].len, 5);
    assert_eq!(page(Some("sub".into()), u64::MAX).0.len(), 1);
}

#[test]
fn a_mod_path_can_never_leave_its_bucket_or_name_an_unportable_file() {
    for bad in [
        "",
        "..",
        "../x",
        "a/../b",
        "/abs",
        "a//b",
        "a/",
        ".hidden",
        "a/.trash-1",
        "con",
        "CON.txt",
        "lpt9.log",
        "conin$",
        "a:b",
        "a\\b",
        "x.",
        "x ",
        "q?",
        "tab\t",
    ] {
        assert!(mod_api::file_path_problem(bad).is_some(), "{bad:?} passed");
    }
    for good in [
        "a",
        "rec/r3/events.pmc",
        "ä ö/ü",
        "console.log",
        "a.b.c",
        "x .y",
    ] {
        assert_eq!(mod_api::file_path_problem(good), None, "{good:?} refused");
    }
}
