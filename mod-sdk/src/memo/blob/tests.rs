use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Memory {
    values: BTreeMap<Vec<u8>, Vec<u8>>,
    leases: BTreeSet<Vec<u8>>,
}
impl Access for Memory {
    fn claim(&mut self, key: &[u8]) -> MemoClaim {
        if let Some(v) = self.values.get(key) {
            MemoClaim::Value(v.clone())
        } else if self.leases.insert(key.to_vec()) {
            MemoClaim::Lease
        } else {
            MemoClaim::Pending
        }
    }
    fn get_many(&mut self, keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>> {
        keys.iter().map(|k| self.values.get(k).cloned()).collect()
    }
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> bool {
        assert!(value.len() <= PAGE_BYTES);
        self.values.insert(key.to_vec(), value);
        self.leases.remove(key);
        true
    }
}

#[test]
fn eviction_claims_one_producer_and_never_returns_a_partial_value() {
    let mut a = Memory::default();
    let key = b"feature";
    let data = vec![7; PAGE_BYTES + 3];
    assert!(matches!(claim(&mut a, key), MemoClaim::Lease));
    assert!(matches!(claim(&mut a, key), MemoClaim::Pending));
    assert!(put(&mut a, key, data.clone()));
    assert_eq!(claim(&mut a, key), MemoClaim::Value(data.clone()));
    a.values.remove(&page_key(key, 1));
    assert!(matches!(claim(&mut a, key), MemoClaim::Lease));
    assert!(matches!(claim(&mut a, key), MemoClaim::Pending));
    assert!(put(&mut a, key, data.clone()));
    assert_eq!(claim(&mut a, key), MemoClaim::Value(data));
    for data in [Vec::new(), vec![8; PAGE_BYTES - 1], vec![9; PAGE_BYTES]] {
        assert!(put(&mut a, key, data.clone()));
        assert_eq!(claim(&mut a, key), MemoClaim::Value(data));
    }
}
