use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct VariantId(pub u16);

impl VariantId {
    pub const NONE: VariantId = VariantId(0);

    #[inline]
    pub fn is_none(self) -> bool {
        self == VariantId::NONE
    }
}

pub type VariantMap = BTreeMap<String, Vec<u8>>;

pub const INFO_DATA_KEY: &str = "petramond:info";

pub const MAX_KEYS: usize = 4;
pub const MAX_KEY_BYTES: usize = 64;
pub const MAX_VALUE_BYTES: usize = 128;
pub const MAX_VARIANTS: usize = u16::MAX as usize;

pub fn valid(data: &VariantMap) -> bool {
    !data.is_empty()
        && data.len() <= MAX_KEYS
        && data.iter().all(|(k, v)| {
            k.len() <= MAX_KEY_BYTES
                && v.len() <= MAX_VALUE_BYTES
                && crate::registry::namespace(k).is_some()
        })
}

pub fn encode(data: &VariantMap) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.push(data.len() as u8);
    for (k, v) in data {
        out.push(k.len() as u8);
        out.extend_from_slice(k.as_bytes());
        out.push(v.len() as u8);
        out.extend_from_slice(v);
    }
    out
}

pub fn decode(bytes: &[u8]) -> Option<VariantMap> {
    let (&count, mut rest) = bytes.split_first()?;
    let mut map = VariantMap::new();
    for _ in 0..count {
        let (&klen, r) = rest.split_first()?;
        if r.len() < klen as usize {
            return None;
        }
        let (kb, r) = r.split_at(klen as usize);
        let key = std::str::from_utf8(kb).ok()?.to_owned();
        let (&vlen, r) = r.split_first()?;
        if r.len() < vlen as usize {
            return None;
        }
        let (vb, r) = r.split_at(vlen as usize);
        if let Some((last, _)) = map.last_key_value() {
            if *last >= key {
                return None;
            }
        }
        map.insert(key, vb.to_vec());
        rest = r;
    }
    if !rest.is_empty() || !valid(&map) {
        return None;
    }
    Some(map)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VariantError {
    Invalid,
    Malformed,
    TableFull,
}

impl std::fmt::Display for VariantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VariantError::Invalid => "item data is empty, over a cap, or has a bare key",
            VariantError::Malformed => "item data blob is malformed",
            VariantError::TableFull => "the item data table is full",
        })
    }
}

impl std::error::Error for VariantError {}

const CHUNK: usize = 1024;
const CHUNKS: usize = MAX_VARIANTS.div_ceil(CHUNK);

struct Row {
    map: Arc<VariantMap>,
    blob: Arc<Vec<u8>>,
}

/// One content registry's variant table (see `ContentRegistry::variants`):
/// append-only rows readers reach WITHOUT a lock — a chunk of rows is
/// published once and never moves, so `get`/`blob` are two acquire loads —
/// and a blob index that only interning locks.
type VariantChunk = OnceLock<Box<[OnceLock<Row>]>>;

pub struct VariantTable {
    chunks: Box<[VariantChunk]>,
    index: RwLock<HashMap<Vec<u8>, u16>>,
}

impl Default for VariantTable {
    fn default() -> Self {
        VariantTable {
            chunks: (0..CHUNKS).map(|_| OnceLock::new()).collect(),
            index: RwLock::new(HashMap::new()),
        }
    }
}

impl VariantTable {
    pub fn intern(&self, data: &VariantMap) -> Result<VariantId, VariantError> {
        if !valid(data) {
            return Err(VariantError::Invalid);
        }
        let blob = encode(data);
        if let Some(&id) = self
            .index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&blob)
        {
            return Ok(VariantId(id));
        }
        let mut index = self.index.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(&id) = index.get(&blob) {
            return Ok(VariantId(id));
        }
        let slot = index.len();
        if slot >= MAX_VARIANTS {
            return Err(VariantError::TableFull);
        }
        let chunk =
            self.chunks[slot / CHUNK].get_or_init(|| (0..CHUNK).map(|_| OnceLock::new()).collect());
        let _ = chunk[slot % CHUNK].set(Row {
            map: Arc::new(data.clone()),
            blob: Arc::new(blob.clone()),
        });
        let id = (slot + 1) as u16;
        index.insert(blob, id);
        Ok(VariantId(id))
    }

    pub fn intern_blob(&self, bytes: &[u8]) -> Result<VariantId, VariantError> {
        self.intern(&decode(bytes).ok_or(VariantError::Malformed)?)
    }

    fn row(&self, id: VariantId) -> Option<&Row> {
        let slot = (id.0 as usize).checked_sub(1)?;
        self.chunks.get(slot / CHUNK)?.get()?[slot % CHUNK].get()
    }

    pub fn contains(&self, data: &VariantMap) -> bool {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(&encode(data))
    }

    pub fn len(&self) -> usize {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: VariantId) -> Option<Arc<VariantMap>> {
        self.row(id).map(|r| r.map.clone())
    }

    pub fn blob(&self, id: VariantId) -> Option<Arc<Vec<u8>>> {
        self.row(id).map(|r| r.blob.clone())
    }
}

#[inline]
fn table() -> &'static VariantTable {
    crate::content::current().variants()
}

pub fn intern(data: &VariantMap) -> Result<VariantId, VariantError> {
    table().intern(data)
}

pub fn intern_blob(bytes: &[u8]) -> Result<VariantId, VariantError> {
    table().intern_blob(bytes)
}

#[cfg(any(test, feature = "test-support"))]
pub fn is_interned_for_test(data: &VariantMap) -> bool {
    table().contains(data)
}

pub fn get(id: VariantId) -> Option<Arc<VariantMap>> {
    table().get(id)
}

pub fn blob(id: VariantId) -> Option<Arc<Vec<u8>>> {
    table().blob(id)
}

pub fn matches(id: VariantId, data: &VariantMap) -> bool {
    match blob(id) {
        Some(b) => b.as_slice() == encode(data),
        None => id.is_none() && data.is_empty(),
    }
}

pub fn value(id: VariantId, key: &str) -> Option<Vec<u8>> {
    get(id).and_then(|m| m.get(key).cloned())
}

pub fn tint(id: VariantId) -> Option<[f32; 3]> {
    let v = value(id, crate::block::TINT_KV_KEY)?;
    let [r, g, b]: [u8; 3] = v.as_slice().try_into().ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

pub const OVERLAY_DATA_KEY: &str = "petramond:overlay";

pub fn overlay_items(id: VariantId) -> Vec<super::ItemType> {
    let Some(v) = value(id, OVERLAY_DATA_KEY) else {
        return Vec::new();
    };
    let Ok(s) = std::str::from_utf8(&v) else {
        return Vec::new();
    };
    s.split(',')
        .filter_map(|name| super::ItemType::by_name(name.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, &[u8])]) -> VariantMap {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_vec()))
            .collect()
    }

    #[test]
    fn overlay_items_resolve_names_in_order_and_skip_unknowns() {
        use super::super::ItemType;
        let id = |val: &[u8]| {
            let mut m = VariantMap::new();
            m.insert(OVERLAY_DATA_KEY.to_string(), val.to_vec());
            intern(&m).unwrap()
        };
        assert_eq!(
            overlay_items(id(b"petramond:stick, petramond:coal")),
            vec![ItemType::Stick, ItemType::Coal]
        );
        assert_eq!(
            overlay_items(id(b"gone:mod_item,petramond:stick")),
            vec![ItemType::Stick],
            "an unknown name is skipped, not an error"
        );
        assert_eq!(overlay_items(id(&[0xFF, 0xFE])), vec![], "non-UTF-8 = none");
        assert_eq!(overlay_items(VariantId::NONE), vec![]);
    }

    #[test]
    fn equal_maps_intern_to_the_same_id_and_unequal_ones_do_not() {
        let a = map(&[("m:tint", &[10, 20, 30])]);
        let b = map(&[("m:tint", &[10, 20, 30])]);
        let c = map(&[("m:tint", &[10, 20, 31])]);
        let ia = intern(&a).unwrap();
        assert_eq!(intern(&b).unwrap(), ia, "byte-equal maps share an id");
        assert_ne!(intern(&c).unwrap(), ia, "one byte off = distinct variant");
        assert_eq!(*get(ia).unwrap(), a);
    }

    #[test]
    fn blobs_round_trip_and_reintern_to_the_same_id() {
        let m = map(&[("m:a", &[1]), ("m:b", &[2, 3])]);
        let id = intern(&m).unwrap();
        let blob = blob(id).unwrap();
        assert_eq!(decode(&blob).unwrap(), m);
        assert_eq!(intern_blob(&blob).unwrap(), id);
    }

    #[test]
    fn invalid_maps_and_malformed_blobs_are_refused() {
        let invalid = Err(VariantError::Invalid);
        assert_eq!(intern(&VariantMap::new()), invalid, "empty = no variant");
        assert_eq!(intern(&map(&[("bare_key", &[1])])), invalid, "bare key");
        assert_eq!(
            intern(&map(&[("m:big", &[0u8; MAX_VALUE_BYTES + 1])])),
            invalid,
            "value cap"
        );
        let over: VariantMap = (0..5).map(|i| (format!("m:k{i}"), vec![0u8])).collect();
        assert_eq!(intern(&over), invalid, "key-count cap");
        assert_eq!(intern_blob(&[1, 3]), Err(VariantError::Malformed));
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[1, 3]), None, "truncated key");
        let mut swapped = vec![2, 3];
        swapped.extend(b"m:b");
        swapped.extend([1, 2, 3]);
        swapped.extend(b"m:a");
        swapped.extend([1, 1]);
        assert_eq!(decode(&swapped), None, "unsorted keys refused");
    }

    #[test]
    fn each_table_numbers_its_own_variants() {
        let (a, b) = (VariantTable::default(), VariantTable::default());
        let red = map(&[("m:tint", &[255, 0, 0])]);
        let blue = map(&[("m:tint", &[0, 0, 255])]);
        let red_in_a = a.intern(&red).unwrap();
        assert_eq!(b.intern(&blue).unwrap(), red_in_a, "both tables start at 1");
        assert_eq!(*a.get(red_in_a).unwrap(), red);
        assert_eq!(*b.get(red_in_a).unwrap(), blue);
        assert_eq!(
            a.get(VariantId(2)),
            None,
            "an id past the table reads nothing"
        );
        assert!(a.contains(&red) && !a.contains(&blue));
    }

    #[test]
    fn a_full_table_refuses_new_maps_and_keeps_serving_old_ones() {
        let table = VariantTable::default();
        let nth = |i: usize| map(&[("m:n", &(i as u32).to_le_bytes())]);
        for i in 0..MAX_VARIANTS {
            table.intern(&nth(i)).unwrap();
        }
        assert_eq!(table.len(), MAX_VARIANTS);
        assert_eq!(
            table.intern(&nth(MAX_VARIANTS)),
            Err(VariantError::TableFull)
        );
        let last = VariantId(MAX_VARIANTS as u16);
        assert_eq!(*table.get(last).unwrap(), nth(MAX_VARIANTS - 1));
        assert_eq!(
            table.intern(&nth(7)).unwrap(),
            VariantId(8),
            "repeats still intern"
        );
    }
}
