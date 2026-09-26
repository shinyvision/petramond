use super::*;

#[derive(Debug, PartialEq)]
struct Counter(u32);

impl KvRecord for Counter {
    const VERSION: u8 = 3;
    fn encode(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Some(Self(u32::from_le_bytes(bytes.try_into().ok()?)))
    }
}

/// Stored as one byte at v1, two at v2, four at v3: each step widens.
#[derive(Debug, PartialEq)]
struct Widened(u32);

impl KvRecord for Widened {
    const VERSION: u8 = 3;
    const OLDEST_VERSION: u8 = 1;
    fn encode(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Some(Self(u32::from_le_bytes(bytes.try_into().ok()?)))
    }
    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        match from {
            1 => Some(u16::from(*bytes.first()?).to_le_bytes().to_vec()),
            2 => Some(
                u32::from(u16::from_le_bytes(bytes.try_into().ok()?))
                    .to_le_bytes()
                    .to_vec(),
            ),
            _ => None,
        }
    }
}

#[test]
fn an_edit_reports_a_change_only_when_the_bytes_differ() {
    let mut held = Held::new(Counter(1));
    assert_eq!(held.edit(|c| c.0), (1, false));
    assert_eq!(held.edit(|c| c.0 = 1), ((), false));
    assert_eq!(held.edit(|c| c.0 = 2), ((), true));
    assert_eq!(unversioned::<Counter>(&held.bytes), Ok(Counter(2)));
    assert_eq!(held.edit(|c| c.0 = 2), ((), false));
}

#[test]
fn an_unreadable_value_is_an_error_never_absent() {
    let mut bytes = versioned(&Counter(9));
    assert_eq!(unversioned::<Counter>(&bytes), Ok(Counter(9)));
    bytes[0] += 1;
    assert_eq!(
        unversioned::<Counter>(&bytes),
        Err(RecordError::Newer {
            found: 4,
            newest: 3
        })
    );
    bytes[0] = 2;
    assert_eq!(
        unversioned::<Counter>(&bytes),
        Err(RecordError::Retired {
            found: 2,
            oldest: 3
        }),
        "no upgrade path declared: older versions are retired"
    );
    assert_eq!(unversioned::<Counter>(&[]), Err(RecordError::Empty));
    assert_eq!(unversioned::<Counter>(&[3, 1]), Err(RecordError::Corrupt));
}

#[test]
fn older_values_migrate_through_every_step() {
    assert_eq!(unversioned::<Widened>(&[1, 7]), Ok(Widened(7)));
    assert_eq!(unversioned::<Widened>(&[2, 7, 1]), Ok(Widened(263)));
    assert_eq!(
        unversioned::<Widened>(&[3, 1, 0, 0, 0]),
        Ok(Widened(1)),
        "the current version needs no step"
    );
    assert_eq!(
        unversioned::<Widened>(&[1]),
        Err(RecordError::Upgrade { from: 1 }),
        "a step that rejects its bytes names itself"
    );
    assert_eq!(
        unversioned::<Widened>(&[0, 1]),
        Err(RecordError::Retired {
            found: 0,
            oldest: 1
        })
    );
}

/// A migrated record is held beside its OLD bytes, so its first update
/// writes it back in the current version even when the value is unchanged.
#[test]
fn a_migrated_record_is_rewritten_by_its_next_update() {
    let stored = vec![1, 7];
    let mut held = Held {
        value: unversioned::<Widened>(&stored).unwrap(),
        bytes: stored,
    };
    assert_eq!(held.edit(|w| w.0), (7, true));
    assert_eq!(held.bytes, versioned(&Widened(7)));
    assert_eq!(held.edit(|w| w.0), (7, false));
}

/// A standalone blob is framed exactly like a stored record, so the same
/// upgrade chain lifts it.
#[test]
fn a_standalone_blob_carries_the_record_framing() {
    let bytes = encode_versioned(&Widened(300));
    assert_eq!(bytes, versioned(&Widened(300)));
    assert_eq!(decode_versioned::<Widened>(&bytes), Ok(Widened(300)));
    assert_eq!(decode_versioned::<Widened>(&[2, 44, 1]), Ok(Widened(300)));
    assert_eq!(decode_versioned::<Widened>(&[]), Err(RecordError::Empty));
}
