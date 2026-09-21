use super::*;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petramond-journal-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn batch(generation: u8) -> Vec<Entry> {
    vec![
        Entry::Region {
            rx: 0,
            rz: -1,
            records: vec![(3, vec![generation; 40]), (9, vec![generation + 1; 12])],
        },
        Entry::File {
            path: "level.dat".into(),
            bytes: vec![generation; 8],
        },
        Entry::File {
            path: "players/ada.dat".into(),
            bytes: vec![generation; 5],
        },
    ]
}

/// What the save holds for the entries of [`batch`].
fn state(dir: &Path) -> [Option<Vec<u8>>; 3] {
    let region = region::RegionReader::open(&region::region_path(&dir.join("region"), 0, -1))
        .ok()
        .and_then(|mut r| r.read_record(3).ok().flatten());
    [
        region,
        std::fs::read(dir.join("level.dat")).ok(),
        std::fs::read(dir.join("players/ada.dat")).ok(),
    ]
}

fn generation(dir: &Path) -> Option<u8> {
    let [region, level, player] = state(dir);
    let (region, level, player) = (region?, level?, player?);
    assert!(
        region[0] == level[0] && level[0] == player[0],
        "a save mixes batches: region {} level {} player {}",
        region[0],
        level[0],
        player[0]
    );
    Some(level[0])
}

#[test]
fn a_committed_batch_is_replayed_whole_after_any_crash_past_the_commit() {
    for crash in [Crash::AfterCommit, Crash::MidApply(1), Crash::MidApply(2)] {
        let dir = temp_dir(&format!("commit-{crash:?}"));
        write(&dir, &batch(1)).unwrap();
        write_crashing(&dir, &batch(2), crash).unwrap();
        assert!(recover(&dir).unwrap(), "{crash:?}");
        assert_eq!(generation(&dir), Some(2), "{crash:?}");
        assert!(!dir.join(NAME).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn a_torn_journal_leaves_the_previous_save() {
    let dir = temp_dir("torn");
    write(&dir, &batch(1)).unwrap();
    write_crashing(&dir, &batch(2), Crash::TornJournal).unwrap();
    assert!(!recover(&dir).unwrap());
    assert_eq!(generation(&dir), Some(1));
    assert!(!dir.join(NAME).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_apply_keeps_the_batch_for_the_next_open() {
    let dir = temp_dir("failed");
    write(&dir, &batch(1)).unwrap();
    block_player_file(&dir, true);
    assert!(write(&dir, &batch(2)).is_err());
    assert!(dir.join(NAME).exists(), "the unfinished batch is retained");
    block_player_file(&dir, false);
    assert!(recover(&dir).unwrap());
    assert_eq!(generation(&dir), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Make the player file of [`batch`] unwritable / writable again.
fn block_player_file(dir: &Path, blocked: bool) {
    let path = dir.join("players/ada.dat");
    if blocked {
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir_all(path.join("in-the-way")).unwrap();
    } else {
        std::fs::remove_dir_all(&path).unwrap();
    }
}

#[test]
fn nothing_is_committed_over_a_batch_that_has_not_landed() {
    let dir = temp_dir("kept");
    write(&dir, &batch(1)).unwrap();
    block_player_file(&dir, true);
    let mut second = batch(2);
    second.insert(
        0,
        Entry::Region {
            rx: 4,
            rz: 4,
            records: vec![(1, vec![2; 6])],
        },
    );
    assert!(write(&dir, &second).is_err());
    let unfinished = std::fs::read(dir.join(NAME)).unwrap();

    assert!(write(&dir, &batch(3)).is_err(), "the held batch goes first");
    assert_eq!(std::fs::read(dir.join(NAME)).unwrap(), unfinished);

    block_player_file(&dir, false);
    write(&dir, &batch(3)).unwrap();
    assert_eq!(generation(&dir), Some(3));
    let only_in_second =
        region::RegionReader::open(&region::region_path(&dir.join("region"), 4, 4))
            .unwrap()
            .read_record(1)
            .unwrap();
    assert_eq!(
        only_in_second,
        Some(vec![2; 6]),
        "the held batch landed too"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replay_never_empties_a_region_it_cannot_read() {
    let dir = temp_dir("unreadable");
    write(&dir, &batch(1)).unwrap();
    let region_file = region::region_path(&dir.join("region"), 0, -1);
    std::fs::write(&region_file, b"rotten").unwrap();
    write_crashing(&dir, &batch(2), Crash::AfterCommit).unwrap();
    assert!(recover(&dir).unwrap());
    assert_eq!(generation(&dir), Some(2));
    assert_eq!(
        std::fs::read(region_file.with_extension("dat.unreadable")).unwrap(),
        b"rotten",
        "the bytes that could not be read are kept"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_journal_cannot_name_a_file_outside_the_world() {
    for path in ["../level.dat", "/etc/level.dat", "players/../../x", ""] {
        let entries = [Entry::File {
            path: path.into(),
            bytes: vec![1],
        }];
        assert_eq!(decode(&encode(&entries)), None, "{path:?}");
    }
    assert!(decode(&encode(&batch(1))).is_some());
}
