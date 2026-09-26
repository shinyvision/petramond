use super::*;

#[derive(Debug, Default, PartialEq)]
struct Pair {
    a: u16,
    b: String,
}
wire_struct!(Pair { a, b });

/// A record as one build writes it...
#[derive(Debug, Default, PartialEq)]
struct Old {
    name: String,
    count: u32,
    unknown: UnknownFields,
}
tagged_record!(Old {
    1 => name,
    2 => count,
});

/// ...and as a later build writes it, with one field more.
#[derive(Debug, Default, PartialEq)]
struct New {
    name: String,
    count: u32,
    extra: Option<Vec<i64>>,
    unknown: UnknownFields,
}
tagged_record!(New {
    1 => name,
    2 => count,
    3 => extra,
});

fn roundtrip<T: Wire + PartialEq + std::fmt::Debug>(value: T) {
    assert_eq!(from_bytes::<T>(&to_bytes(&value)).as_ref(), Some(&value));
}

#[test]
fn values_roundtrip() {
    roundtrip(7u8);
    roundtrip(-3i32);
    roundtrip(i64::MIN);
    roundtrip(1.5f64);
    roundtrip(true);
    roundtrip(String::from("petramond:torch"));
    roundtrip(vec![1u16, 2, 3]);
    roundtrip(Some(vec![Some(4u32), None]));
    roundtrip(BTreeMap::from([("a".to_owned(), 1u8), ("b".to_owned(), 2)]));
    roundtrip((9u16, String::from("x")));
    roundtrip(WorldPos::new(-1.25, 64.0, 1e9));
    roundtrip(Vec3::new(0.5, -2.0, 3.0));
    roundtrip(IVec3::new(-7, 0, 9));
    roundtrip(Blob16(vec![1, 2, 3]));
    roundtrip(Pair {
        a: 300,
        b: "pair".into(),
    });
}

#[test]
fn malformed_values_are_rejected() {
    assert_eq!(from_bytes::<bool>(&[2]), None, "a bool is 0 or 1");
    assert_eq!(from_bytes::<String>(&[2, 0, 0, 0, 0xFF, 0xFE]), None);
    assert_eq!(from_bytes::<u32>(&[1, 2, 3]), None, "truncated");
    assert_eq!(from_bytes::<u8>(&[1, 2]), None, "trailing bytes");
    let mut repeated = to_bytes(&BTreeMap::from([("k".to_owned(), 1u8)]));
    repeated[0] = 2;
    repeated.extend(to_bytes(&("k".to_owned(), 2u8)));
    assert_eq!(from_bytes::<BTreeMap<String, u8>>(&repeated), None);
}

/// The two directions of a positional struct are one statement: the
/// fields go out in the listed order.
#[test]
fn a_wire_struct_writes_its_fields_in_order() {
    let bytes = to_bytes(&Pair {
        a: 0x0102,
        b: "z".into(),
    });
    assert_eq!(bytes, [0x02, 0x01, 1, 0, 0, 0, b'z']);
}

/// Older records read in a newer build: the missing field is its default.
#[test]
fn a_field_the_record_lacks_reads_as_its_default() {
    let old = Old {
        name: "sheep".into(),
        count: 3,
        unknown: UnknownFields::new(),
    };
    let new: New = from_bytes(&to_bytes(&old)).expect("decodes");
    assert_eq!(
        new,
        New {
            name: "sheep".into(),
            count: 3,
            extra: None,
            unknown: UnknownFields::new(),
        }
    );
}

/// Newer records read in an older build: the field it does not know is
/// kept and written back exactly as it came.
#[test]
fn a_field_this_build_does_not_know_is_kept_and_written_back() {
    let new = New {
        name: "sheep".into(),
        count: 3,
        extra: Some(vec![-1, 2]),
        unknown: UnknownFields::new(),
    };
    let bytes = to_bytes(&new);
    let old: Old = from_bytes(&bytes).expect("decodes");
    assert_eq!(old.unknown.keys().copied().collect::<Vec<_>>(), [3]);
    assert_eq!(to_bytes(&old), bytes, "re-encodes byte for byte");
    assert_eq!(from_bytes::<New>(&to_bytes(&old)), Some(new));
}

#[test]
fn a_repeated_tag_or_a_field_that_misfits_its_bytes_is_malformed() {
    let mut buf = Vec::new();
    let mut w = TaggedWriter::new(&mut buf);
    w.field(1, &String::from("a"));
    w.field(1, &String::from("b"));
    w.finish();
    assert_eq!(from_bytes::<Old>(&buf), None, "repeated tag");

    let mut buf = Vec::new();
    let mut w = TaggedWriter::new(&mut buf);
    w.field(2, &7u64);
    w.finish();
    assert_eq!(from_bytes::<Old>(&buf), None, "a u64 where a u32 goes");
}
