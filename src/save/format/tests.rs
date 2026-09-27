use super::*;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("petramond-format-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const DEMO: Format = Format::new("demo record", 3, &[double, mark]);

fn double(body: &[u8]) -> Result<Vec<u8>, RecordError> {
    Ok(body.iter().flat_map(|&b| [b, b]).collect())
}

fn mark(body: &[u8]) -> Result<Vec<u8>, RecordError> {
    if body.is_empty() {
        return Err(RecordError::corrupt("demo record", "body", 0));
    }
    Ok([body, &[0xFF]].concat())
}

#[test]
fn the_chain_runs_every_step_from_the_record_version() {
    assert_eq!(DEMO.oldest(), 1);
    assert_eq!(&*DEMO.upgrade(1, &[1, 2]).unwrap(), &[1, 1, 2, 2, 0xFF]);
    assert_eq!(&*DEMO.upgrade(2, &[1, 2]).unwrap(), &[1, 2, 0xFF]);
    assert!(matches!(
        DEMO.upgrade(3, &[1, 2]).unwrap(),
        Cow::Borrowed(&[1, 2])
    ));
}

#[test]
fn versions_outside_the_chain_and_failing_steps_are_typed_errors() {
    assert_eq!(
        DEMO.upgrade(4, &[]).err(),
        Some(RecordError::Newer {
            format: "demo record",
            found: 4,
            newest: 3,
        })
    );
    assert_eq!(
        DEMO.upgrade(0, &[]).err(),
        Some(RecordError::Retired {
            format: "demo record",
            found: 0,
            oldest: 1,
        })
    );
    assert_eq!(
        DEMO.upgrade(1, &[]).err(),
        Some(RecordError::corrupt("demo record", "body", 0)),
        "a step's own error surfaces unchanged"
    );
}

#[test]
fn a_u32_header_record_splits_before_upgrading() {
    let mut bytes = 2u32.to_le_bytes().to_vec();
    bytes.push(7);
    assert_eq!(&*DEMO.upgrade_u32_record(&bytes).unwrap(), &[7, 0xFF]);
    assert_eq!(
        DEMO.upgrade_u32_record(&[1, 0]).err(),
        Some(RecordError::corrupt("demo record", "version header", 0))
    );
}

#[test]
fn only_newer_build_errors_forbid_overwriting() {
    assert!(RecordError::Newer {
        format: "x",
        found: 2,
        newest: 1
    }
    .is_from_newer_build());
    assert!(RecordError::UnknownPayload {
        format: "x",
        flags: 1
    }
    .is_from_newer_build());
    assert!(!RecordError::corrupt("x", "y", 0).is_from_newer_build());
    assert!(!RecordError::Retired {
        format: "x",
        found: 0,
        oldest: 1
    }
    .is_from_newer_build());
}

#[test]
fn a_fresh_world_is_stamped_with_this_build() {
    let dir = temp_dir("fresh");
    assert_eq!(prepare_world(&dir).unwrap(), None);
    assert_eq!(read_stamp(&dir).unwrap(), Some(WorldFormat::current()));
    assert_eq!(
        prepare_world(&dir).unwrap(),
        Some(WorldFormat::current()),
        "a second open finds its own stamp"
    );
    assert!(!dir.join("backup").exists(), "nothing to migrate");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_world_from_a_newer_build_is_refused_untouched() {
    let dir = temp_dir("newer");
    let newer = WorldFormat {
        section: WorldFormat::current().section + 1,
        ..WorldFormat::current()
    };
    let json = serde_json::to_string(&newer).unwrap();
    std::fs::write(dir.join(STAMP), &json).unwrap();
    let err = prepare_world(&dir).expect_err("refused");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    let inner = err
        .get_ref()
        .and_then(|e| e.downcast_ref::<RecordError>())
        .expect("the typed reason rides the io error");
    assert!(inner.is_from_newer_build());
    assert_eq!(std::fs::read_to_string(dir.join(STAMP)).unwrap(), json);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_older_world_is_backed_up_then_restamped() {
    let dir = temp_dir("older");
    let older = WorldFormat {
        section: super::super::codec::SECTION.oldest(),
        ..WorldFormat::current()
    };
    std::fs::write(dir.join(STAMP), serde_json::to_string(&older).unwrap()).unwrap();
    std::fs::write(dir.join("level.dat"), b"level").unwrap();
    std::fs::create_dir_all(dir.join("players")).unwrap();
    std::fs::write(dir.join("players").join("a.dat"), b"player").unwrap();

    assert_eq!(prepare_world(&dir).unwrap(), Some(older));
    assert_eq!(read_stamp(&dir).unwrap(), Some(WorldFormat::current()));
    let backup = dir.join("backup").join(format!(
        "format-s{}-l{}-p{}",
        older.section, older.level, older.player
    ));
    assert_eq!(std::fs::read(backup.join("level.dat")).unwrap(), b"level");
    assert_eq!(
        std::fs::read(backup.join("players").join("a.dat")).unwrap(),
        b"player"
    );
    assert_eq!(read_stamp(&backup).unwrap(), Some(older));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_world_older_than_the_chain_is_refused() {
    let dir = temp_dir("retired");
    let retired = WorldFormat {
        section: super::super::codec::SECTION.oldest() - 1,
        ..WorldFormat::current()
    };
    std::fs::write(dir.join(STAMP), serde_json::to_string(&retired).unwrap()).unwrap();
    assert!(prepare_world(&dir).is_err());
    assert!(!dir.join("backup").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unparseable_stamp_is_an_error_not_a_legacy_world() {
    let dir = temp_dir("garbled");
    std::fs::write(dir.join(STAMP), b"{").unwrap();
    assert_eq!(
        prepare_world(&dir).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(std::fs::read(dir.join(STAMP)).unwrap(), b"{");
    let _ = std::fs::remove_dir_all(&dir);
}
