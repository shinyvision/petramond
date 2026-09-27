use super::*;
use petramond_util::test_dirs::TestScratchDir;
use std::fs;
use std::path::PathBuf;

/// The scratch dir must outlive the test body: dropping it removes the file.
fn temp_path(tag: &str) -> (TestScratchDir, PathBuf) {
    let dir = TestScratchDir::new(&format!("region-{tag}"));
    let path = dir.join("region.dat");
    (dir, path)
}

fn read(path: &Path, lidx: u16) -> Option<Vec<u8>> {
    RegionReader::open(path)
        .expect("region opens")
        .read_record(lidx)
        .expect("record reads")
}

/// A v2 file laid out by hand: the contiguous header table, then the bodies.
fn write_v2(path: &Path, records: &[(u16, &[u8])]) {
    let mut out = Vec::new();
    out.extend(MAGIC_V2.to_le_bytes());
    out.extend(VERSION_V2.to_le_bytes());
    out.extend((records.len() as u16).to_le_bytes());
    for (lidx, body) in records {
        out.extend(lidx.to_le_bytes());
        out.extend((body.len() as u32).to_le_bytes());
    }
    for (_, body) in records {
        out.extend(*body);
    }
    fs::write(path, out).unwrap();
}

#[test]
fn empty_merge_is_a_noop() {
    let (_dir, path) = temp_path("empty");
    merge_region(&path, [], MergePolicy::Durable).expect("empty merge");
    assert!(
        !path.exists(),
        "an empty replacement set must not create a region file"
    );
}

#[test]
fn merge_preserves_untouched_records() {
    let (_dir, path) = temp_path("preserve");
    merge_region(
        &path,
        [(9, vec![1, 2, 3]), (2, vec![4, 5])],
        MergePolicy::Durable,
    )
    .expect("initial write");
    assert_eq!(read_region_indices(&path).expect("index"), vec![2, 9]);

    merge_region(&path, [(9, vec![8, 7, 6, 5])], MergePolicy::Durable).expect("merge");
    assert_eq!(read(&path, 2), Some(vec![4, 5]));
    assert_eq!(read(&path, 9), Some(vec![8, 7, 6, 5]));
    assert_eq!(read(&path, 100), None);
}

/// The point of v3: replacing one record appends it and a new index; every
/// byte already in the file stays where it was, except the header slot.
#[test]
fn a_merge_appends_instead_of_rewriting_the_file() {
    let (_dir, path) = temp_path("append");
    let big: Vec<(u16, Vec<u8>)> = (0..50u16).map(|i| (i, vec![i as u8; 1000])).collect();
    merge_region(&path, big, MergePolicy::Durable).expect("initial write");
    let before = fs::read(&path).unwrap();

    merge_region(&path, [(7, vec![0xEE; 10])], MergePolicy::Durable).expect("merge");
    let after = fs::read(&path).unwrap();
    assert_eq!(
        after.len(),
        before.len() + 10 + 4 + 50 * INDEX_ENTRY_BYTES,
        "one body and one index appended, nothing copied"
    );
    assert_eq!(
        &after[DATA_START as usize..before.len()],
        &before[DATA_START as usize..],
        "bodies and the old index are untouched"
    );
    assert_eq!(read(&path, 7), Some(vec![0xEE; 10]));
    assert_eq!(read(&path, 8), Some(vec![8; 1000]));
}

/// A reader opened before an append keeps reading what it indexed: nothing
/// it points at is overwritten.
#[test]
fn an_open_reader_survives_an_append() {
    let (_dir, path) = temp_path("reader");
    merge_region(&path, [(1, vec![1; 20])], MergePolicy::Durable).unwrap();
    let mut early = RegionReader::open(&path).unwrap();
    merge_region(&path, [(1, vec![2; 30])], MergePolicy::Durable).unwrap();
    assert_eq!(early.read_record(1).unwrap(), Some(vec![1; 20]));
    assert_eq!(read(&path, 1), Some(vec![2; 30]));
}

/// A torn (or never flushed) newest slot fails its checksum, and so does a
/// slot whose index never reached the disk: the reader falls back to the
/// previous slot, which still names the state before the merge.
#[test]
fn a_torn_slot_or_index_falls_back_to_the_previous_state() {
    let (_dir, path) = temp_path("torn");
    merge_region(&path, [(3, vec![1; 8])], MergePolicy::Durable).unwrap();
    merge_region(&path, [(3, vec![2; 8])], MergePolicy::Durable).unwrap();
    // The first write was a compaction into slot 0; the append used slot 1.
    let mut bytes = fs::read(&path).unwrap();
    bytes[Slot::offset(1) as usize + 3] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();
    assert_eq!(read(&path, 3), Some(vec![1; 8]), "torn slot");

    merge_region(&path, [(3, vec![4; 8])], MergePolicy::Durable).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();
    assert_eq!(read(&path, 3), Some(vec![1; 8]), "torn index");
}

/// Bytes a crashed append left past the index are garbage no slot names:
/// readers ignore them and the next merge appends after them.
#[test]
fn trailing_bytes_from_a_crashed_append_are_ignored() {
    let (_dir, path) = temp_path("trailing");
    merge_region(&path, [(5, vec![5; 16])], MergePolicy::Durable).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend([0xAB; 100]);
    fs::write(&path, &bytes).unwrap();
    assert_eq!(read(&path, 5), Some(vec![5; 16]));
    merge_region(&path, [(6, vec![6; 16])], MergePolicy::Durable).unwrap();
    assert_eq!(read(&path, 5), Some(vec![5; 16]));
    assert_eq!(read(&path, 6), Some(vec![6; 16]));
}

/// Rewriting the same records over and over compacts once the garbage
/// outweighs the live data: the file stays bounded by a small multiple of
/// what it holds.
#[test]
fn garbage_is_compacted_away() {
    let (_dir, path) = temp_path("compact");
    let body = |n: u8| vec![n; 64 << 10];
    for round in 0..40u8 {
        merge_region(
            &path,
            [(1, body(round)), (2, body(round.wrapping_add(1)))],
            MergePolicy::Durable,
        )
        .unwrap();
        let live = 2 * (64 << 10) as u64;
        let len = fs::metadata(&path).unwrap().len();
        assert!(
            len <= DATA_START + 2 * live.max(COMPACT_FLOOR) + (64 << 10) * 2 + 1024,
            "round {round}: {len} bytes for {live} live"
        );
    }
    assert_eq!(read(&path, 1), Some(body(39)));
    assert_eq!(read(&path, 2), Some(body(40)));
}

#[test]
fn a_v2_file_reads_and_becomes_v3_on_its_first_merge() {
    let (_dir, path) = temp_path("v2");
    write_v2(&path, &[(2, &[0xBB; 20]), (1, &[0xAA; 10])]);
    assert_eq!(read_region_indices(&path).unwrap(), vec![1, 2]);
    assert_eq!(read(&path, 1), Some(vec![0xAA; 10]));

    merge_region(&path, [(3, vec![0xCC; 5])], MergePolicy::Durable).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(&bytes[0..4], &MAGIC_V3.to_le_bytes(), "rewritten as v3");
    assert_eq!(read(&path, 1), Some(vec![0xAA; 10]));
    assert_eq!(read(&path, 2), Some(vec![0xBB; 20]));
    assert_eq!(read(&path, 3), Some(vec![0xCC; 5]));
}

#[test]
fn an_unreadable_region_fails_a_durable_merge_and_is_left_alone() {
    let (_dir, path) = temp_path("unreadable");
    fs::write(&path, b"not a region").unwrap();
    let err = merge_region(&path, [(1, vec![1])], MergePolicy::Durable).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert_eq!(fs::read(&path).unwrap(), b"not a region");

    merge_region(&path, [(1, vec![1])], MergePolicy::Rebuildable).expect("starts fresh");
    assert_eq!(read_region_indices(&path).unwrap(), vec![1]);
}

#[test]
fn v1_interleaved_files_are_rejected() {
    let (_dir, path) = temp_path("v1");
    let mut out = Vec::new();
    out.extend(MAGIC_V2.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // the old interleaved version
    out.extend(1u16.to_le_bytes());
    out.extend(7u16.to_le_bytes());
    out.extend(3u32.to_le_bytes());
    out.extend([1, 2, 3]);
    fs::write(&path, out).unwrap();
    match RegionReader::open(&path) {
        Err(err) => assert_eq!(err.kind(), io::ErrorKind::InvalidData),
        Ok(_) => panic!("v1 interleaved region must be rejected"),
    }
}

#[test]
fn a_slot_roundtrips_and_rejects_torn_or_empty_bytes() {
    let slot = Slot {
        seq: 7,
        index_offset: 4096,
        index_len: 18,
        index_hash: 0xDEAD_BEEF_0123,
    };
    let bytes = slot.to_bytes();
    assert_eq!(Slot::from_bytes(&bytes), Some(slot));
    let mut torn = bytes;
    torn[9] ^= 1;
    assert_eq!(Slot::from_bytes(&torn), None);
    assert_eq!(Slot::from_bytes(&[0u8; SLOT_BYTES]), None);
}
