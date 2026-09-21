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

#[test]
fn an_edit_reports_a_change_only_when_the_bytes_differ() {
    let mut held = Held::new(Counter(1));
    assert_eq!(held.edit(|c| c.0), (1, false));
    assert_eq!(held.edit(|c| c.0 = 1), ((), false));
    assert_eq!(held.edit(|c| c.0 = 2), ((), true));
    assert_eq!(unversioned::<Counter>(&held.bytes), Some(Counter(2)));
    assert_eq!(held.edit(|c| c.0 = 2), ((), false));
}

#[test]
fn another_version_reads_as_absent() {
    let mut bytes = versioned(&Counter(9));
    assert_eq!(unversioned::<Counter>(&bytes), Some(Counter(9)));
    bytes[0] += 1;
    assert_eq!(unversioned::<Counter>(&bytes), None);
    assert_eq!(unversioned::<Counter>(&[]), None);
}
