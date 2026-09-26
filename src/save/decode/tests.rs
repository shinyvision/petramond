use super::*;

/// [`super::decode_record`] through the identity palette.
fn decode_record(
    dir: &Path,
    pos: SectionPos,
    store: SectionStore,
    bytes: std::io::Result<Option<Vec<u8>>>,
) -> SectionRecord {
    super::decode_record(dir, pos, store, bytes, &Palette::identity()).0
}

fn temp_world_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("petramond-decodetest-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn decoders_publish_in_request_order_and_shutdown_drains_accepted_jobs() {
    let (tx, rx) = mpsc::channel();
    let (columns, _) = mpsc::channel();
    let mut decoders = Decoders::new(
        temp_world_dir("order"),
        Arc::new(Palette::identity()),
        tx,
        columns,
    );
    for x in 0..32 {
        decoders.submit(DecodeJob::Section {
            pos: SectionPos::new(x, 0, 0),
            store: SectionStore::ExploredCache,
            bytes: Ok(None),
        });
    }
    drop(decoders);
    let results: Vec<_> = rx.try_iter().collect();
    assert_eq!(results.len(), 32);
    for (x, result) in results.iter().enumerate() {
        assert_eq!(result.loaded.pos.cx, x as i32);
        assert!(matches!(result.loaded.record, SectionRecord::Absent));
        assert!(result.kept.is_empty());
    }
}

/// A corrupt authoritative record is reported with its reason — never as
/// absent — and its bytes are kept in quarantine before anything can
/// replace them. Reading the same bad bytes again keeps one copy; different
/// bad bytes for the same section get a sibling.
#[test]
fn a_corrupt_authoritative_record_is_quarantined_not_absent() {
    let dir = temp_world_dir("quarantine");
    let pos = SectionPos::new(33, 2, -1);
    let bad = vec![0x78, 0x9c, 1, 2, 3];
    let record = decode_record(
        &dir,
        pos,
        SectionStore::Authoritative,
        Ok(Some(bad.clone())),
    );
    let SectionRecord::Unreadable(unreadable) = record else {
        panic!("a corrupt record is unreadable, not absent or decoded");
    };
    assert!(matches!(unreadable.error, RecordError::Corrupt { .. }));
    let kept = unreadable.quarantined.clone().expect("bytes kept");
    assert_eq!(std::fs::read(&kept).unwrap(), bad);
    assert!(kept.starts_with(dir.join("quarantine").join("region")));
    assert!(
        !unreadable.must_not_overwrite(),
        "a kept corrupt record may be regenerated over"
    );

    let again = decode_record(&dir, pos, SectionStore::Authoritative, Ok(Some(bad)));
    let SectionRecord::Unreadable(again) = again else {
        panic!("still unreadable");
    };
    assert_eq!(
        again.quarantined.as_ref(),
        Some(&kept),
        "one copy per content"
    );

    let other = decode_record(&dir, pos, SectionStore::Authoritative, Ok(Some(vec![9])));
    let SectionRecord::Unreadable(other) = other else {
        panic!("still unreadable");
    };
    let sibling = other.quarantined.expect("different bytes kept too");
    assert_ne!(sibling, kept, "different bytes, new copy");
    assert_eq!(std::fs::read(&kept).unwrap(), vec![0x78, 0x9c, 1, 2, 3]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A record a newer build wrote is intact data this build cannot represent:
/// it is kept AND must not be overwritten. A read failure leaves nothing to
/// keep, so it must not be overwritten either.
#[test]
fn newer_and_unreadable_records_must_not_be_overwritten() {
    let dir = temp_world_dir("newer");
    let pos = SectionPos::new(0, 0, 0);
    let newer = codec::deflate(&[codec::SECTION.current as u8 + 1, 0, 0, 0]);
    let SectionRecord::Unreadable(u) =
        decode_record(&dir, pos, SectionStore::Authoritative, Ok(Some(newer)))
    else {
        panic!("a newer record is unreadable");
    };
    assert!(matches!(u.error, RecordError::Newer { .. }));
    assert!(u.quarantined.is_some() && u.must_not_overwrite());

    let io = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let SectionRecord::Unreadable(u) =
        decode_record(&dir, pos, SectionStore::Authoritative, Err(io))
    else {
        panic!("a failed read is unreadable, not absent");
    };
    assert!(u.quarantined.is_none() && u.must_not_overwrite());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The explored cache is rebuildable: an unreadable cache record is
/// reported but never quarantined.
#[test]
fn an_unreadable_cache_record_is_not_quarantined() {
    let dir = temp_world_dir("cache");
    let record = decode_record(
        &dir,
        SectionPos::new(1, 1, 1),
        SectionStore::ExploredCache,
        Ok(Some(vec![1, 2, 3])),
    );
    let SectionRecord::Unreadable(u) = record else {
        panic!("unreadable, not absent");
    };
    assert!(u.quarantined.is_none());
    assert!(!dir.join("quarantine").exists());
}
